//! Postgres restart resilience.
//!
//! While 500 RPS of data-plane traffic flows, restart the Postgres container.
//! The gateway should keep serving (config is cached in memory). Admin writes
//! during the outage should fail loudly with 5xx; once Postgres recovers,
//! admin writes must succeed again within 30 seconds.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use tokio::sync::Mutex;
use tokio::time::{sleep_until, Instant as TokioInstant};

use crate::chaos::{fmt_count, AdminClient, Args, Report};

const SCENARIO: &str = "pg-restart";

pub async fn run(admin: &AdminClient, args: &Args) -> Result<Report> {
    let pg = match args.pg_container.as_deref() {
        Some(c) => c.to_string(),
        None => {
            return Ok(Report::fail(
                SCENARIO,
                "missing --pg-container; pass the postgres docker container name",
            ))
        }
    };

    let svc_id = admin
        .create_service("chaos-pg", &args.mock_upstream_url, 0, 5_000)
        .await?;
    let route_id = admin
        .create_route(svc_id, &["/chaos-pg"], &[], true)
        .await?;

    let url = format!("{}/chaos-pg", args.proxy_url.trim_end_matches('/'));
    let client = reqwest::Client::builder()
        .pool_max_idle_per_host(64)
        .timeout(Duration::from_secs(5))
        .build()?;

    let phase = Arc::new(std::sync::atomic::AtomicU8::new(0));
    let pre_ok = Arc::new(AtomicU64::new(0));
    let pre_err = Arc::new(AtomicU64::new(0));
    let mid_ok = Arc::new(AtomicU64::new(0));
    let mid_err = Arc::new(AtomicU64::new(0));
    let post_ok = Arc::new(AtomicU64::new(0));
    let post_err = Arc::new(AtomicU64::new(0));
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));

    let load_handle = {
        let phase = phase.clone();
        let stop = stop.clone();
        let pre_ok = pre_ok.clone();
        let pre_err = pre_err.clone();
        let mid_ok = mid_ok.clone();
        let mid_err = mid_err.clone();
        let post_ok = post_ok.clone();
        let post_err = post_err.clone();
        let client = client.clone();
        let url = url.clone();
        tokio::spawn(async move {
            let rps = 500u64;
            let interval = Duration::from_nanos(1_000_000_000 / rps);
            let connections = 64usize;
            let (tx, rx) = tokio::sync::mpsc::channel::<()>(connections * 4);
            let rx = Arc::new(Mutex::new(rx));
            let mut handles = Vec::new();
            for _ in 0..connections {
                let rx = rx.clone();
                let client = client.clone();
                let url = url.clone();
                let phase = phase.clone();
                let pre_ok = pre_ok.clone();
                let pre_err = pre_err.clone();
                let mid_ok = mid_ok.clone();
                let mid_err = mid_err.clone();
                let post_ok = post_ok.clone();
                let post_err = post_err.clone();
                handles.push(tokio::spawn(async move {
                    loop {
                        {
                            let mut g = rx.lock().await;
                            if g.recv().await.is_none() {
                                break;
                            }
                        }
                        let p = phase.load(Ordering::Relaxed);
                        let ok_c;
                        let err_c;
                        match p {
                            0 => {
                                ok_c = &pre_ok;
                                err_c = &pre_err;
                            }
                            1 => {
                                ok_c = &mid_ok;
                                err_c = &mid_err;
                            }
                            _ => {
                                ok_c = &post_ok;
                                err_c = &post_err;
                            }
                        }
                        match client.get(&url).send().await {
                            Ok(resp) => {
                                let s = resp.status();
                                let _ = resp.bytes().await;
                                if s.is_success() {
                                    ok_c.fetch_add(1, Ordering::Relaxed);
                                } else {
                                    err_c.fetch_add(1, Ordering::Relaxed);
                                }
                            }
                            Err(_) => {
                                err_c.fetch_add(1, Ordering::Relaxed);
                            }
                        }
                    }
                }));
            }
            let start = TokioInstant::now();
            let mut i: u32 = 0;
            while !stop.load(Ordering::Relaxed) {
                let target = start + interval * i;
                sleep_until(target).await;
                i = i.wrapping_add(1);
                if tx.send(()).await.is_err() {
                    break;
                }
            }
            drop(tx);
            for h in handles {
                let _ = h.await;
            }
        })
    };

    // Pre-restart phase.
    tokio::time::sleep(Duration::from_secs(30)).await;

    // Trigger the restart, switch phase to "mid".
    phase.store(1, Ordering::Relaxed);
    tracing::info!(container = %pg, "restarting postgres");
    let restart_started = Instant::now();
    let restart_status = std::process::Command::new("docker")
        .args(["restart", &pg])
        .status();
    match restart_status {
        Ok(s) if s.success() => {}
        Ok(s) => {
            stop.store(true, Ordering::Relaxed);
            let _ = load_handle.await;
            return Ok(Report::fail(
                SCENARIO,
                format!("docker restart failed: exit {s:?}"),
            ));
        }
        Err(e) => {
            stop.store(true, Ordering::Relaxed);
            let _ = load_handle.await;
            return Ok(Report::fail(
                SCENARIO,
                format!("docker restart errored: {e}"),
            ));
        }
    }

    // During the outage, admin writes should fail. We try one and record.
    let admin_outage_status = match admin
        .create_service("chaos-pg-outage-probe", &args.mock_upstream_url, 0, 5_000)
        .await
    {
        Ok(id) => {
            // It actually came back already? Clean up.
            let _ = admin.delete_service(id).await;
            "succeeded (unexpected)".to_string()
        }
        Err(e) => format!("failed (expected): {e}"),
    };

    // Wait out the outage window — 30s of degraded admin access is the ceiling.
    tokio::time::sleep(Duration::from_secs(20)).await;

    // Recovery phase.
    phase.store(2, Ordering::Relaxed);

    // Poll admin writes with a 30s cap.
    let recovery_deadline = Instant::now() + Duration::from_secs(30);
    let mut admin_recovered_at: Option<Duration> = None;
    while Instant::now() < recovery_deadline {
        tokio::time::sleep(Duration::from_secs(2)).await;
        if let Ok(id) = admin
            .create_service("chaos-pg-recover-probe", &args.mock_upstream_url, 0, 5_000)
            .await
        {
            admin_recovered_at = Some(restart_started.elapsed());
            let _ = admin.delete_service(id).await;
            break;
        }
    }

    // Wind down.
    tokio::time::sleep(Duration::from_secs(5)).await;
    stop.store(true, Ordering::Relaxed);
    let _ = load_handle.await;

    let _ = admin.delete_route(route_id).await;
    let _ = admin.delete_service(svc_id).await;

    let pre_ok_n = pre_ok.load(Ordering::Relaxed);
    let pre_err_n = pre_err.load(Ordering::Relaxed);
    let mid_ok_n = mid_ok.load(Ordering::Relaxed);
    let mid_err_n = mid_err.load(Ordering::Relaxed);
    let post_ok_n = post_ok.load(Ordering::Relaxed);
    let post_err_n = post_err.load(Ordering::Relaxed);

    let admin_recovered = admin_recovered_at.is_some();
    let mid_success_rate = if mid_ok_n + mid_err_n == 0 {
        1.0
    } else {
        mid_ok_n as f64 / (mid_ok_n + mid_err_n) as f64
    };
    // Cached config means in-flight data-plane requests should remain healthy.
    let pass = admin_recovered && mid_success_rate >= 0.99 && pre_err_n == 0 && post_err_n == 0;

    let summary = format!(
        "data: pre={}/{} mid={}/{} post={}/{} · admin_recovered={}",
        fmt_count(pre_ok_n),
        fmt_count(pre_err_n),
        fmt_count(mid_ok_n),
        fmt_count(mid_err_n),
        fmt_count(post_ok_n),
        fmt_count(post_err_n),
        admin_recovered_at
            .map(|d| format!("{}s", d.as_secs()))
            .unwrap_or_else(|| "never".into())
    );
    let r = if pass {
        Report::pass(SCENARIO, summary)
    } else {
        Report::fail(SCENARIO, summary)
    };
    Ok(r.detail(format!("admin during outage: {admin_outage_status}")))
}
