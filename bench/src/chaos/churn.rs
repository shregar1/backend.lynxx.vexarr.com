//! Service churn: create/use/delete a service every 5s while a steady stream
//! of background traffic flows through a stable route.
//!
//! Catches three classes of bug at once:
//!   - leaked health-registry entries (we query /health at the end and assert
//!     no chaos-churn-* entries with an open circuit remain),
//!   - route table races on concurrent create/delete + lookup,
//!   - admin/data-plane priority inversion (background traffic shouldn't
//!     stutter while admin writes land).

use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use tokio::time::{sleep_until, Instant as TokioInstant};
use uuid::Uuid;

use crate::chaos::{fmt_count, AdminClient, Args, Report};

const SCENARIO: &str = "churn";

pub async fn run(admin: &AdminClient, args: &Args) -> Result<Report> {
    let duration = args.duration_or(Duration::from_secs(120));
    let bg_rps = args.rps_or(500);

    // Disable any global rate-limit plugins for the duration. They cap
    // throughput below our background RPS, causing false 429 errors that
    // look like upstream failures. We restore enabled state on exit.
    let global_ratelimit_ids: Vec<Uuid> = admin
        .list_plugins()
        .await
        .unwrap_or_default()
        .into_iter()
        .filter(|p| {
            p.name == "rate-limit"
                && p.service_id.is_none()
                && p.route_id.is_none()
                && p.enabled
        })
        .map(|p| p.id)
        .collect();
    for &id in &global_ratelimit_ids {
        let _ = admin.set_plugin_enabled(id, false).await;
        tracing::info!(%id, "disabled global rate-limit plugin for churn test");
    }

    // Record circuits already open before we start so we don't count them
    // as leaked at the end.
    let pre_existing_open: HashSet<Uuid> = admin
        .health()
        .await
        .unwrap_or_default()
        .into_iter()
        .filter(|e| e.circuit == "Open")
        .map(|e| e.service_id)
        .collect();

    // Stable background route.
    let bg_svc = admin
        .create_service("chaos-churn-bg", &args.mock_upstream_url, 0, 5_000)
        .await?;
    let bg_route = admin
        .create_route(bg_svc, &["/chaos-churn-bg"], &[], true)
        .await?;

    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let bg_ok = Arc::new(AtomicU64::new(0));
    let bg_err = Arc::new(AtomicU64::new(0));

    let bg_handle = {
        let stop = stop.clone();
        let bg_ok = bg_ok.clone();
        let bg_err = bg_err.clone();
        let url = format!("{}/chaos-churn-bg", args.proxy_url.trim_end_matches('/'));
        tokio::spawn(async move {
            let client = reqwest::Client::builder()
                .pool_max_idle_per_host(64)
                .timeout(Duration::from_secs(5))
                .build()
                .unwrap();
            let rps = bg_rps;
            let interval = Duration::from_nanos(1_000_000_000 / rps);
            let start = TokioInstant::now();
            let mut i: u32 = 0;
            while !stop.load(Ordering::Relaxed) {
                let target = start + interval * i;
                sleep_until(target).await;
                i = i.wrapping_add(1);
                let url = url.clone();
                let bg_ok = bg_ok.clone();
                let bg_err = bg_err.clone();
                let client = client.clone();
                tokio::spawn(async move {
                    match client.get(&url).send().await {
                        Ok(resp) => {
                            let s = resp.status();
                            let _ = resp.bytes().await;
                            if s.is_success() {
                                bg_ok.fetch_add(1, Ordering::Relaxed);
                            } else {
                                bg_err.fetch_add(1, Ordering::Relaxed);
                            }
                        }
                        Err(_) => {
                            bg_err.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                });
            }
        })
    };

    // Churn loop. Each tick: create svc + route, send 10 reqs, delete.
    let churn_start = std::time::Instant::now();
    let mut tick = 0u64;
    let client = reqwest::Client::builder()
        .pool_max_idle_per_host(8)
        .timeout(Duration::from_secs(5))
        .build()?;
    let churn_err = Arc::new(AtomicU64::new(0));

    while churn_start.elapsed() < duration {
        tick += 1;
        let name = format!("chaos-churn-{tick}");
        let path = format!("/chaos-churn-{tick}");
        let svc = match admin.create_service(&name, &args.mock_upstream_url, 0, 5_000).await {
            Ok(id) => id,
            Err(e) => {
                tracing::warn!(error = %e, "churn create_service failed");
                churn_err.fetch_add(1, Ordering::Relaxed);
                tokio::time::sleep(Duration::from_secs(5)).await;
                continue;
            }
        };
        let route = match admin.create_route(svc, &[&path], &[], true).await {
            Ok(id) => id,
            Err(e) => {
                tracing::warn!(error = %e, "churn create_route failed");
                churn_err.fetch_add(1, Ordering::Relaxed);
                let _ = admin.delete_service(svc).await;
                tokio::time::sleep(Duration::from_secs(5)).await;
                continue;
            }
        };

        let url = format!("{}{}", args.proxy_url.trim_end_matches('/'), path);
        for _ in 0..10 {
            match client.get(&url).send().await {
                Ok(resp) => {
                    let s = resp.status();
                    let _ = resp.bytes().await;
                    if !s.is_success() {
                        churn_err.fetch_add(1, Ordering::Relaxed);
                    }
                }
                Err(_) => {
                    churn_err.fetch_add(1, Ordering::Relaxed);
                }
            }
        }

        let _ = admin.delete_route(route).await;
        let _ = admin.delete_service(svc).await;
        tokio::time::sleep(Duration::from_secs(5)).await;
    }

    stop.store(true, Ordering::Relaxed);
    let _ = bg_handle.await;
    // Allow in-flight bg requests to settle.
    tokio::time::sleep(Duration::from_millis(500)).await;

    let bg_ok_n = bg_ok.load(Ordering::Relaxed);
    let bg_err_n = bg_err.load(Ordering::Relaxed);
    let churn_err_n = churn_err.load(Ordering::Relaxed);
    let total = bg_ok_n + bg_err_n + 10 * tick;
    let total_err = bg_err_n + churn_err_n;
    let err_rate = if total == 0 { 0.0 } else { 100.0 * total_err as f64 / total as f64 };

    // Leaked open circuits? Skip bg_svc (may have transient blips) and any
    // circuits that were already open before this scenario ran.
    let mut leaked = 0u64;
    if let Ok(entries) = admin.health().await {
        for e in entries {
            if e.service_id == bg_svc {
                continue;
            }
            if pre_existing_open.contains(&e.service_id) {
                continue;
            }
            if e.circuit == "Open" {
                leaked += 1;
            }
        }
    }

    let _ = admin.delete_route(bg_route).await;
    let _ = admin.delete_service(bg_svc).await;

    // Restore rate-limit plugins we disabled at the start.
    for &id in &global_ratelimit_ids {
        let _ = admin.set_plugin_enabled(id, true).await;
        tracing::info!(%id, "re-enabled global rate-limit plugin");
    }

    let pass = err_rate < 0.1 && leaked == 0;
    let summary = format!(
        "{} ticks · err_rate={:.3}% · leaked_open_circuits={}",
        fmt_count(tick),
        err_rate,
        leaked
    );
    let r = if pass {
        Report::pass(SCENARIO, summary)
    } else {
        Report::fail(SCENARIO, summary)
    };
    Ok(r.detail(format!(
        "bg ok={bg_ok_n} bg err={bg_err_n} churn err={churn_err_n} rps={bg_rps}"
    )))
}
