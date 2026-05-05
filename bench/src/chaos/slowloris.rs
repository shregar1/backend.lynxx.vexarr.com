//! Slowloris: 100 raw TCP clients trickling a request byte-by-byte while a
//! second pool of fast clients hammers a normal route.
//!
//! Pass criteria: every fast request succeeds and p50 stays under 50ms while
//! 100 slow connections are open. The slow clients don't need to "succeed"
//! (the gateway is allowed to time them out) — what matters is that they
//! don't starve the event loop.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;
use tokio::sync::Mutex;

use crate::chaos::{fmt_us, new_hist, AdminClient, Args, Report};

const SCENARIO: &str = "slowloris";

pub async fn run(admin: &AdminClient, args: &Args) -> Result<Report> {
    let duration = args.duration_or(Duration::from_secs(30));

    // Need a real route for the fast clients. Reuse mock upstream.
    let svc_id = admin
        .create_service("chaos-slowloris", &args.mock_upstream_url, 0, 5_000)
        .await?;
    let route_id = admin
        .create_route(svc_id, &["/chaos-slow"], &[], true)
        .await?;

    // Parse host:port from proxy_url for raw-TCP connection.
    let proxy_addr = parse_host_port(&args.proxy_url)?;

    let slow_handles = (0..100)
        .map(|_| {
            let addr = proxy_addr.clone();
            let dur = duration;
            tokio::spawn(async move {
                let _ = run_slow_client(&addr, dur).await;
            })
        })
        .collect::<Vec<_>>();

    // Give the slow clients a moment to establish.
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Fast pool — 100 simple GETs concurrently with the slow loris.
    let url = format!("{}/chaos-slow", args.proxy_url.trim_end_matches('/'));
    let client = reqwest::Client::builder()
        .pool_max_idle_per_host(32)
        .timeout(Duration::from_secs(5))
        .build()?;
    let hist = Arc::new(Mutex::new(new_hist()));
    let ok = Arc::new(AtomicU64::new(0));
    let err = Arc::new(AtomicU64::new(0));

    let mut fast = Vec::with_capacity(100);
    for _ in 0..100 {
        let client = client.clone();
        let url = url.clone();
        let hist = hist.clone();
        let ok = ok.clone();
        let err = err.clone();
        fast.push(tokio::spawn(async move {
            let start = Instant::now();
            match client.get(&url).send().await {
                Ok(resp) => {
                    let s = resp.status();
                    let _ = resp.bytes().await;
                    let us = start.elapsed().as_micros() as u64;
                    if s.is_success() {
                        ok.fetch_add(1, Ordering::Relaxed);
                        let mut h = hist.lock().await;
                        let _ = h.record(us.max(1));
                    } else {
                        err.fetch_add(1, Ordering::Relaxed);
                    }
                }
                Err(_) => {
                    err.fetch_add(1, Ordering::Relaxed);
                }
            }
        }));
    }
    for h in fast {
        let _ = h.await;
    }
    // Let the slow clients drain (they self-terminate after `duration`).
    for h in slow_handles {
        let _ = h.await;
    }

    let _ = admin.delete_route(route_id).await;
    let _ = admin.delete_service(svc_id).await;

    let okc = ok.load(Ordering::Relaxed);
    let errc = err.load(Ordering::Relaxed);
    let hist = Arc::try_unwrap(hist).map(|m| m.into_inner()).ok();
    let p50 = hist.as_ref().map(|h| h.value_at_quantile(0.50)).unwrap_or(0);
    let p50_ms = p50 as f64 / 1_000.0;

    let summary = format!(
        "{}/100 fast ok · p50={} during slow",
        okc,
        fmt_us(p50)
    );
    let passed = errc == 0 && okc == 100 && p50_ms < 50.0;
    let r = if passed {
        Report::pass(SCENARIO, summary)
    } else {
        Report::fail(SCENARIO, summary)
    };
    Ok(r.detail(format!("100 slow TCP clients held open for {}s", duration.as_secs())))
}

fn parse_host_port(url: &str) -> Result<String> {
    // Strip scheme.
    let rest = url
        .trim_end_matches('/')
        .trim_start_matches("http://")
        .trim_start_matches("https://");
    // Drop any path.
    let host_port = rest.split('/').next().unwrap_or(rest);
    if !host_port.contains(':') {
        anyhow::bail!("expected host:port in proxy URL, got `{url}`");
    }
    Ok(host_port.to_string())
}

async fn run_slow_client(addr: &str, total: Duration) -> Result<()> {
    let mut s = TcpStream::connect(addr).await?;
    // Send one byte every ~300ms. The exact request line doesn't matter — we
    // never finish it. Cap the sent bytes at the total duration / 300ms.
    let line = b"GET / HTTP/1.1\r\nHost: x\r\n";
    let per_byte = Duration::from_millis(300);
    let max_bytes = (total.as_millis() / per_byte.as_millis()) as usize;
    for &b in line.iter().take(max_bytes) {
        if s.write_all(&[b]).await.is_err() {
            return Ok(()); // gateway closed the connection — fine
        }
        let _ = s.flush().await;
        tokio::time::sleep(per_byte).await;
    }
    Ok(())
}
