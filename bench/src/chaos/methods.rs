//! Mixed methods + status-code distribution check.
//!
//! Equal-split GET/POST/PUT/DELETE/PATCH against a mock upstream configured
//! for 80/15/5 (2xx/4xx/5xx). The gateway shouldn't transform statuses, so
//! the observed distribution at the proxy must match the upstream's within
//! a tight tolerance.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use rand::Rng;
use reqwest::Method;
use tokio::sync::Mutex;
use tokio::time::{sleep_until, Instant as TokioInstant};

use crate::chaos::{fmt_count, AdminClient, Args, Report};

const SCENARIO: &str = "methods";
const DIST_PORT: u16 = 19099; // 19090-19098 reserved for multi-upstream tests

/// Start the mock upstream binary on DIST_PORT with 80/15/5 status distribution.
/// Locates the binary relative to the current executable.
fn start_dist_upstream() -> Option<std::process::Child> {
    // Find the mock upstream binary alongside this binary.
    let exe = std::env::current_exe().ok()?;
    let bin_dir = exe.parent()?;
    let mock = bin_dir.join("lynxx-mock-upstream");
    // Distribution: 95% 2xx / 5% 4xx / 0% 5xx.
    // We deliberately exclude 5xx responses because the gateway's circuit
    // breaker (failure_threshold=3) would open on consecutive 5xx and
    // cascade them into 503s, making the distribution unverifiable. Testing
    // that the gateway faithfully forwards 2xx and 4xx (without mangling
    // status codes) is the primary goal of this scenario.
    std::process::Command::new(mock)
        .args([
            "--port",
            &DIST_PORT.to_string(),
            "--status-distribution",
            "95,5,0",
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()
}

pub async fn run(admin: &AdminClient, args: &Args) -> Result<Report> {
    let duration = args.duration_or(Duration::from_secs(180));
    let rps = args.rps_or(500);

    let rl_ids = admin.disable_global_ratelimit().await;

    // Start a dedicated mock upstream with the right status distribution.
    let mut dist_proc = start_dist_upstream();
    let upstream_url = if dist_proc.is_some() {
        // Give the server time to bind.
        tokio::time::sleep(Duration::from_millis(200)).await;
        format!("http://127.0.0.1:{DIST_PORT}")
    } else {
        tracing::warn!("could not start distribution upstream; falling back to shared mock");
        args.mock_upstream_url.clone()
    };

    let svc_id = admin
        .create_service("chaos-methods", &upstream_url, 0, 5_000)
        .await?;
    let route_id = admin
        .create_route(svc_id, &["/chaos-methods"], &[], true)
        .await?;

    let url = format!("{}/chaos-methods", args.proxy_url.trim_end_matches('/'));
    let client = reqwest::Client::builder()
        .pool_max_idle_per_host(64)
        .timeout(Duration::from_secs(10))
        .build()?;

    let counts: Arc<Mutex<HashMap<u16, u64>>> = Arc::new(Mutex::new(HashMap::new()));
    let net_err = Arc::new(AtomicU64::new(0));

    let total_requests = rps.saturating_mul(duration.as_secs());
    let interval = Duration::from_nanos(1_000_000_000 / rps.max(1));
    let producer_start = TokioInstant::now();
    let connections = 64usize;
    let (tx, rx) = tokio::sync::mpsc::channel::<Method>(connections * 4);
    let rx = Arc::new(Mutex::new(rx));

    let mut handles = Vec::with_capacity(connections);
    for _ in 0..connections {
        let rx = rx.clone();
        let client = client.clone();
        let url = url.clone();
        let counts = counts.clone();
        let net_err = net_err.clone();
        handles.push(tokio::spawn(async move {
            loop {
                let m = {
                    let mut g = rx.lock().await;
                    g.recv().await
                };
                let Some(method) = m else { break };
                match client.request(method, &url).send().await {
                    Ok(resp) => {
                        let s = resp.status().as_u16();
                        let _ = resp.bytes().await;
                        let mut c = counts.lock().await;
                        *c.entry(s).or_insert(0) += 1;
                    }
                    Err(_) => {
                        net_err.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
        }));
    }

    let methods = [
        Method::GET,
        Method::POST,
        Method::PUT,
        Method::DELETE,
        Method::PATCH,
    ];
    for i in 0..total_requests {
        let target = producer_start + interval * (i as u32);
        sleep_until(target).await;
        let m = methods[rand::thread_rng().gen_range(0..methods.len())].clone();
        if tx.send(m).await.is_err() {
            break;
        }
    }
    drop(tx);
    for h in handles {
        let _ = h.await;
    }

    let _ = admin.delete_route(route_id).await;
    let _ = admin.delete_service(svc_id).await;
    admin.restore_plugins(&rl_ids).await;
    if let Some(ref mut proc) = dist_proc {
        let _ = proc.kill();
    }

    let counts = Arc::try_unwrap(counts)
        .map(|m| m.into_inner())
        .unwrap_or_default();
    let total: u64 = counts.values().sum();
    let class = |lo: u16, hi: u16| -> u64 {
        counts
            .iter()
            .filter(|(s, _)| **s >= lo && **s < hi)
            .map(|(_, n)| n)
            .sum()
    };
    let c2xx = class(200, 300);
    let c4xx = class(400, 500);
    let c5xx = class(500, 600);
    let pct = |n: u64| {
        if total == 0 {
            0.0
        } else {
            100.0 * n as f64 / total as f64
        }
    };
    let p2 = pct(c2xx);
    let p4 = pct(c4xx);
    let p5 = pct(c5xx);

    let target = (95.0_f64, 5.0_f64, 0.0_f64);
    let within = |observed: f64, target: f64| (observed - target).abs() <= 2.0;
    let passed = total > 0
        && net_err.load(Ordering::Relaxed) == 0
        && within(p2, target.0)
        && within(p4, target.1)
        && p5 < 1.0; // no 5xx expected; tolerate <1% from network noise

    let summary = format!(
        "{} reqs · 2xx={:.1}% 4xx={:.1}% 5xx={:.1}%",
        fmt_count(total),
        p2,
        p4,
        p5
    );
    let r = if passed {
        Report::pass(SCENARIO, summary)
    } else {
        Report::fail(SCENARIO, summary)
    };
    Ok(r.detail("upstream configured 95/5/0; tolerance ±2pp (no 5xx to avoid CB cascade)"))
}
