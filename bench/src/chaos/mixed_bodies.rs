//! Mixed body sizes: 70% GET, 20% POST 1KB, 9% POST 100KB, 1% POST 5MB.
//!
//! Targets a realistic web/api mix where most requests are reads but a small
//! tail carries non-trivial payloads. Buffer pools, header-list reuse and
//! body streaming all get exercised here.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use rand::Rng;
use reqwest::Method;
use tokio::sync::Mutex;
use tokio::time::{sleep_until, Instant as TokioInstant};

use crate::chaos::{fmt_count, fmt_us, new_hist, AdminClient, Args, Report};

const SCENARIO: &str = "mixed-bodies";

pub async fn run(admin: &AdminClient, args: &Args) -> Result<Report> {
    let duration = args.duration_or(Duration::from_secs(300));
    let rps = args.rps_or(1000);

    // Set up a service+route pointing at the mock upstream.
    let svc_id = admin
        .create_service("chaos-mixed-bodies", &args.mock_upstream_url, 0, 5_000)
        .await?;
    let route_id = admin
        .create_route(svc_id, &["/chaos-mixed"], &[], true)
        .await?;
    let cleanup = scopeguard(admin.clone(), svc_id, route_id);

    let url = format!("{}/chaos-mixed", args.proxy_url.trim_end_matches('/'));
    let client = reqwest::Client::builder()
        .pool_max_idle_per_host(64)
        .timeout(Duration::from_secs(15))
        .build()?;

    let body_1k: Arc<Vec<u8>> = Arc::new(vec![b'x'; 1024]);
    let body_100k: Arc<Vec<u8>> = Arc::new(vec![b'x'; 100 * 1024]);
    let body_5m: Arc<Vec<u8>> = Arc::new(vec![b'x'; 5 * 1024 * 1024]);

    let hist = Arc::new(Mutex::new(new_hist()));
    let ok = Arc::new(AtomicU64::new(0));
    let err = Arc::new(AtomicU64::new(0));

    let total_requests = rps.saturating_mul(duration.as_secs());
    let interval = Duration::from_nanos(1_000_000_000 / rps.max(1));
    let producer_start = TokioInstant::now();
    let connections = 64usize;
    let (tx, rx) = tokio::sync::mpsc::channel::<Job>(connections * 4);
    let rx = Arc::new(Mutex::new(rx));

    let mut handles = Vec::with_capacity(connections);
    for _ in 0..connections {
        let rx = rx.clone();
        let client = client.clone();
        let url = url.clone();
        let hist = hist.clone();
        let ok = ok.clone();
        let err = err.clone();
        let b1 = body_1k.clone();
        let b100 = body_100k.clone();
        let b5 = body_5m.clone();
        handles.push(tokio::spawn(async move {
            loop {
                let job = {
                    let mut g = rx.lock().await;
                    g.recv().await
                };
                let Some(job) = job else { break };
                let start = Instant::now();
                let mut req = client.request(job.method.clone(), &url);
                match job.body {
                    BodyKind::None => {}
                    BodyKind::K1 => req = req.body((*b1).clone()),
                    BodyKind::K100 => req = req.body((*b100).clone()),
                    BodyKind::M5 => req = req.body((*b5).clone()),
                }
                let result = req.send().await;
                let elapsed_us = start.elapsed().as_micros() as u64;
                match result {
                    Ok(resp) => {
                        let s = resp.status();
                        let _ = resp.bytes().await;
                        if s.is_success() {
                            ok.fetch_add(1, Ordering::Relaxed);
                            let mut h = hist.lock().await;
                            let _ = h.record(elapsed_us.max(1));
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

    for i in 0..total_requests {
        let target = producer_start + interval * (i as u32);
        sleep_until(target).await;
        let job = pick_job();
        if tx.send(job).await.is_err() {
            break;
        }
    }
    drop(tx);
    for h in handles {
        let _ = h.await;
    }

    let hist = Arc::try_unwrap(hist).map(|m| m.into_inner()).ok();
    let okc = ok.load(Ordering::Relaxed);
    let errc = err.load(Ordering::Relaxed);
    let p99 = hist.as_ref().map(|h| h.value_at_quantile(0.99)).unwrap_or(0);
    let p99_ms = p99 as f64 / 1_000.0;

    drop(cleanup);

    let summary = format!(
        "{} reqs · {} errs · p99={}",
        fmt_count(okc + errc),
        fmt_count(errc),
        fmt_us(p99)
    );
    let passed = errc == 0 && p99_ms < 100.0;
    let r = if passed {
        Report::pass(SCENARIO, summary)
    } else {
        Report::fail(SCENARIO, summary)
    };
    Ok(r.detail(format!("p99 budget 100ms; observed {p99_ms:.1}ms")))
}

#[derive(Clone, Copy)]
enum BodyKind {
    None,
    K1,
    K100,
    M5,
}

#[derive(Clone)]
struct Job {
    method: Method,
    body: BodyKind,
}

fn pick_job() -> Job {
    let r = rand::thread_rng().gen_range(0..1000);
    if r < 700 {
        Job { method: Method::GET, body: BodyKind::None }
    } else if r < 900 {
        Job { method: Method::POST, body: BodyKind::K1 }
    } else if r < 990 {
        Job { method: Method::POST, body: BodyKind::K100 }
    } else {
        Job { method: Method::POST, body: BodyKind::M5 }
    }
}

// -- cleanup -----------------------------------------------------------------

struct ScopeGuard {
    admin: AdminClient,
    svc: uuid::Uuid,
    route: uuid::Uuid,
}

impl Drop for ScopeGuard {
    fn drop(&mut self) {
        let admin = self.admin.clone();
        let svc = self.svc;
        let route = self.route;
        // Best-effort cleanup; spawn-and-forget so Drop stays sync.
        tokio::spawn(async move {
            let _ = admin.delete_route(route).await;
            let _ = admin.delete_service(svc).await;
        });
    }
}

fn scopeguard(admin: AdminClient, svc: uuid::Uuid, route: uuid::Uuid) -> ScopeGuard {
    ScopeGuard { admin, svc, route }
}
