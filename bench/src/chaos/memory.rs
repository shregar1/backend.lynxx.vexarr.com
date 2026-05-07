//! Memory leak detection: 10 minutes @ 1000 RPS, sample RSS every 30s.
//!
//! Pass criteria: final RSS - initial RSS < 50 MB. The first sample is taken
//! after a 30s warmup so the steady-state working set is fully populated
//! before we anchor the baseline.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use tokio::sync::Mutex;
use tokio::time::{sleep_until, Instant as TokioInstant};

use crate::chaos::{find_gateway_pid, fmt_count, rss_kb, AdminClient, Args, Report};

const SCENARIO: &str = "memory";

pub async fn run(admin: &AdminClient, args: &Args) -> Result<Report> {
    let duration = args.duration_or(Duration::from_secs(600));
    let rps = args.rps_or(1000);

    let pid = match find_gateway_pid() {
        Some(p) => p,
        None => {
            return Ok(Report::fail(
                SCENARIO,
                "could not locate lynxx-gateway via pgrep",
            ))
        }
    };

    let rl_ids = admin.disable_global_ratelimit().await;
    let svc_id = admin
        .create_service("chaos-memory", &args.mock_upstream_url, 0, 5_000)
        .await?;
    let route_id = admin
        .create_route(svc_id, &["/chaos-mem"], &[], true)
        .await?;

    let url = format!("{}/chaos-mem", args.proxy_url.trim_end_matches('/'));
    let client = reqwest::Client::builder()
        .pool_max_idle_per_host(64)
        .timeout(Duration::from_secs(5))
        .build()?;

    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let ok = Arc::new(AtomicU64::new(0));
    let err = Arc::new(AtomicU64::new(0));

    let load_handle = {
        let stop = stop.clone();
        let ok = ok.clone();
        let err = err.clone();
        let client = client.clone();
        let url = url.clone();
        tokio::spawn(async move {
            let interval = Duration::from_nanos(1_000_000_000 / rps.max(1));
            let connections = 64usize;
            let (tx, rx) = tokio::sync::mpsc::channel::<()>(connections * 4);
            let rx = Arc::new(Mutex::new(rx));
            let mut handles = Vec::new();
            for _ in 0..connections {
                let rx = rx.clone();
                let client = client.clone();
                let url = url.clone();
                let ok = ok.clone();
                let err = err.clone();
                handles.push(tokio::spawn(async move {
                    loop {
                        {
                            let mut g = rx.lock().await;
                            if g.recv().await.is_none() {
                                break;
                            }
                        }
                        match client.get(&url).send().await {
                            Ok(resp) => {
                                let s = resp.status();
                                let _ = resp.bytes().await;
                                if s.is_success() {
                                    ok.fetch_add(1, Ordering::Relaxed);
                                } else {
                                    err.fetch_add(1, Ordering::Relaxed);
                                }
                            }
                            Err(_) => {
                                err.fetch_add(1, Ordering::Relaxed);
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

    // Warmup before the baseline RSS sample.
    tokio::time::sleep(Duration::from_secs(30)).await;
    let baseline = rss_kb(pid).unwrap_or(0);
    let mut samples = vec![(Duration::from_secs(0), baseline)];
    tracing::info!(
        scenario = SCENARIO,
        baseline_kb = baseline,
        "baseline RSS captured"
    );

    let test_start = Instant::now();
    while test_start.elapsed() < duration {
        tokio::time::sleep(Duration::from_secs(30)).await;
        let elapsed = test_start.elapsed();
        if let Some(rss) = rss_kb(pid) {
            samples.push((elapsed, rss));
            tracing::info!(
                scenario = SCENARIO,
                rss_kb = rss,
                elapsed_s = elapsed.as_secs(),
                "rss sample"
            );
        }
    }

    stop.store(true, Ordering::Relaxed);
    let _ = load_handle.await;

    let _ = admin.delete_route(route_id).await;
    let _ = admin.delete_service(svc_id).await;
    admin.restore_plugins(&rl_ids).await;

    let final_rss = samples.last().map(|(_, r)| *r).unwrap_or(baseline);
    let growth_kb = final_rss.saturating_sub(baseline);
    let growth_mb = growth_kb as f64 / 1024.0;
    let okc = ok.load(Ordering::Relaxed);
    let errc = err.load(Ordering::Relaxed);

    let pass = errc == 0 && growth_mb < 50.0;
    let summary = format!(
        "{} reqs · RSS {:.1} -> {:.1} MB ({:+.1} MB)",
        fmt_count(okc + errc),
        baseline as f64 / 1024.0,
        final_rss as f64 / 1024.0,
        growth_mb,
    );
    let mut r = if pass {
        Report::pass(SCENARIO, summary)
    } else {
        Report::fail(SCENARIO, summary)
    };
    r = r.detail(format!(
        "budget 50 MB; observed {:+.1} MB over {}s",
        growth_mb,
        duration.as_secs()
    ));
    for (t, k) in samples.iter().take(10) {
        r = r.detail(format!(
            "  t={}s rss={:.1} MB",
            t.as_secs(),
            *k as f64 / 1024.0
        ));
    }
    Ok(r)
}
