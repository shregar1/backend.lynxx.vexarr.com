//! Stack five plugins on a single route and verify the chain survives load.
//!
//! The mock upstream runs in `--echo` mode so we can inspect a sample
//! response and confirm the JWT plugin injected `X-Jwt-Claims` (i.e. the
//! whole chain executed in order). Verifying every response would dominate
//! the test runtime; sampling a few is enough for chain correctness.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use serde_json::json;
use tokio::sync::Mutex;
use tokio::time::{sleep_until, Instant as TokioInstant};

use crate::chaos::{fmt_count, AdminClient, Args, Report};

const SCENARIO: &str = "plugin-chain";

// HS256 token signed with the secret below; sub=chaos, iss=lynxx-chaos.
// Generated once and embedded so the test is hermetic. If the JWT plugin
// validation rules change, regenerate this with `cargo run -p lynxx-bench
// --bin lynxx-chaos -- ...` after updating the constant.
const JWT_SECRET: &str = "chaos-test-secret-please-do-not-use-in-prod";

pub async fn run(admin: &AdminClient, args: &Args) -> Result<Report> {
    let duration = args.duration_or(Duration::from_secs(60));
    let rps = args.rps_or(1000);

    let svc_id = admin
        .create_service("chaos-plugin-chain", &args.mock_upstream_url, 0, 5_000)
        .await?;
    let route_id = admin
        .create_route(svc_id, &["/chaos-pchain"], &[], true)
        .await?;

    // Five plugins. Configs are intentionally permissive so the chain succeeds
    // for every request — we're testing chain plumbing, not per-plugin policy.
    let mut plugin_ids = Vec::new();
    plugin_ids.push(
        admin
            .install_plugin(
                "jwt",
                json!({ "secret": JWT_SECRET, "algorithms": ["HS256"] }),
                None,
                Some(route_id),
            )
            .await?,
    );
    plugin_ids.push(
        admin
            .install_plugin(
                "rate-limit",
                json!({ "rps": 10_000, "burst": 20_000 }),
                None,
                Some(route_id),
            )
            .await?,
    );
    plugin_ids.push(
        admin
            .install_plugin(
                "cors",
                json!({ "allow_origins": ["*"], "allow_methods": ["GET", "POST"] }),
                None,
                Some(route_id),
            )
            .await?,
    );
    plugin_ids.push(
        admin
            .install_plugin("logging", json!({ "level": "info" }), None, Some(route_id))
            .await?,
    );
    plugin_ids.push(
        admin
            .install_plugin(
                "request-transformer",
                json!({ "add_headers": { "X-Chaos-Test": "1" } }),
                None,
                Some(route_id),
            )
            .await?,
    );

    let token = make_jwt(JWT_SECRET);
    let url = format!("{}/chaos-pchain", args.proxy_url.trim_end_matches('/'));
    let client = reqwest::Client::builder()
        .pool_max_idle_per_host(64)
        .timeout(Duration::from_secs(10))
        .build()?;

    // Sample one request first to verify the chain produced X-Jwt-Claims in
    // the upstream's view (we use the mock upstream's --echo body for this).
    let sample = client
        .get(&url)
        .bearer_auth(&token)
        .send()
        .await?;
    let sample_status = sample.status();
    let sample_body: serde_json::Value = sample.json().await.unwrap_or(json!({}));
    let saw_claims = sample_body
        .get("headers")
        .and_then(|h| h.as_object())
        .map(|m| m.keys().any(|k| k.eq_ignore_ascii_case("x-jwt-claims")))
        .unwrap_or(false);

    if !sample_status.is_success() {
        cleanup(admin, route_id, svc_id, &plugin_ids).await;
        return Ok(Report::fail(
            SCENARIO,
            format!("sample request failed: {sample_status}"),
        ));
    }

    let total_requests = rps.saturating_mul(duration.as_secs());
    let interval = Duration::from_nanos(1_000_000_000 / rps.max(1));
    let connections = 64usize;
    let (tx, rx) = tokio::sync::mpsc::channel::<()>(connections * 4);
    let rx = Arc::new(Mutex::new(rx));
    let ok = Arc::new(AtomicU64::new(0));
    let err = Arc::new(AtomicU64::new(0));

    let mut handles = Vec::new();
    for _ in 0..connections {
        let rx = rx.clone();
        let client = client.clone();
        let url = url.clone();
        let token = token.clone();
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
                match client.get(&url).bearer_auth(&token).send().await {
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
    for i in 0..total_requests {
        let target = start + interval * (i as u32);
        sleep_until(target).await;
        if tx.send(()).await.is_err() {
            break;
        }
    }
    drop(tx);
    for h in handles {
        let _ = h.await;
    }

    cleanup(admin, route_id, svc_id, &plugin_ids).await;

    let okc = ok.load(Ordering::Relaxed);
    let errc = err.load(Ordering::Relaxed);
    let pass = errc == 0 && saw_claims;
    let summary = format!(
        "{} ok · {} err · X-Jwt-Claims={}",
        fmt_count(okc),
        fmt_count(errc),
        if saw_claims { "yes" } else { "no" }
    );
    let r = if pass {
        Report::pass(SCENARIO, summary)
    } else {
        Report::fail(SCENARIO, summary)
    };
    Ok(r.detail("plugins: jwt + rate-limit + cors + logging + request-transformer"))
}

async fn cleanup(admin: &AdminClient, route: uuid::Uuid, svc: uuid::Uuid, plugins: &[uuid::Uuid]) {
    for p in plugins {
        let _ = admin.delete_plugin(*p).await;
    }
    let _ = admin.delete_route(route).await;
    let _ = admin.delete_service(svc).await;
}

/// Hand-roll a minimal HS256 JWT so this scenario doesn't pull in an extra
/// dependency. Header is constant; payload carries `sub` + `iss` + a far-future
/// `exp`; signature is HMAC-SHA256(b64url(header)+"."+b64url(payload), secret).
fn make_jwt(secret: &str) -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let exp = now + 3600;
    let header = b64u(br#"{"alg":"HS256","typ":"JWT"}"#);
    let payload_json = format!(
        r#"{{"sub":"chaos","iss":"lynxx-chaos","iat":{now},"exp":{exp}}}"#
    );
    let payload = b64u(payload_json.as_bytes());
    let signing_input = format!("{header}.{payload}");
    let sig = hmac_sha256(secret.as_bytes(), signing_input.as_bytes());
    let sig_b64 = b64u(&sig);
    format!("{signing_input}.{sig_b64}")
}

fn b64u(bytes: &[u8]) -> String {
    // url-safe base64 without padding, hand-rolled.
    const ALPHA: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity(bytes.len() * 4 / 3 + 4);
    let mut i = 0;
    while i + 3 <= bytes.len() {
        let n = (bytes[i] as u32) << 16 | (bytes[i + 1] as u32) << 8 | (bytes[i + 2] as u32);
        out.push(ALPHA[((n >> 18) & 0x3f) as usize] as char);
        out.push(ALPHA[((n >> 12) & 0x3f) as usize] as char);
        out.push(ALPHA[((n >> 6) & 0x3f) as usize] as char);
        out.push(ALPHA[(n & 0x3f) as usize] as char);
        i += 3;
    }
    let rem = bytes.len() - i;
    if rem == 1 {
        let n = (bytes[i] as u32) << 16;
        out.push(ALPHA[((n >> 18) & 0x3f) as usize] as char);
        out.push(ALPHA[((n >> 12) & 0x3f) as usize] as char);
    } else if rem == 2 {
        let n = (bytes[i] as u32) << 16 | (bytes[i + 1] as u32) << 8;
        out.push(ALPHA[((n >> 18) & 0x3f) as usize] as char);
        out.push(ALPHA[((n >> 12) & 0x3f) as usize] as char);
        out.push(ALPHA[((n >> 6) & 0x3f) as usize] as char);
    }
    out
}

fn hmac_sha256(key: &[u8], msg: &[u8]) -> [u8; 32] {
    // Tiny pure-rust HMAC-SHA256. Kept inline to avoid adding `hmac`/`sha2`
    // to bench/Cargo.toml just for this one helper.
    const BLOCK: usize = 64;
    let mut k = [0u8; BLOCK];
    if key.len() > BLOCK {
        let h = sha256(key);
        k[..32].copy_from_slice(&h);
    } else {
        k[..key.len()].copy_from_slice(key);
    }
    let mut ipad = [0u8; BLOCK];
    let mut opad = [0u8; BLOCK];
    for i in 0..BLOCK {
        ipad[i] = k[i] ^ 0x36;
        opad[i] = k[i] ^ 0x5c;
    }
    let mut inner = Vec::with_capacity(BLOCK + msg.len());
    inner.extend_from_slice(&ipad);
    inner.extend_from_slice(msg);
    let inner_hash = sha256(&inner);
    let mut outer = Vec::with_capacity(BLOCK + 32);
    outer.extend_from_slice(&opad);
    outer.extend_from_slice(&inner_hash);
    sha256(&outer)
}

fn sha256(data: &[u8]) -> [u8; 32] {
    // Reference SHA-256 (FIPS 180-4). Adequate for test-only signing.
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
        0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
        0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
        0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
        0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
        0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
        0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
    ];
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
    ];
    let bit_len = (data.len() as u64).wrapping_mul(8);
    let mut buf = data.to_vec();
    buf.push(0x80);
    while buf.len() % 64 != 56 {
        buf.push(0);
    }
    buf.extend_from_slice(&bit_len.to_be_bytes());
    for chunk in buf.chunks_exact(64) {
        let mut w = [0u32; 64];
        for (i, c) in chunk.chunks_exact(4).enumerate() {
            w[i] = u32::from_be_bytes([c[0], c[1], c[2], c[3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16].wrapping_add(s0).wrapping_add(w[i - 7]).wrapping_add(s1);
        }
        let (mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh) =
            (h[0], h[1], h[2], h[3], h[4], h[5], h[6], h[7]);
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ (!e & g);
            let t1 = hh.wrapping_add(s1).wrapping_add(ch).wrapping_add(K[i]).wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let mj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(mj);
            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
        h[5] = h[5].wrapping_add(f);
        h[6] = h[6].wrapping_add(g);
        h[7] = h[7].wrapping_add(hh);
    }
    let mut out = [0u8; 32];
    for (i, v) in h.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&v.to_be_bytes());
    }
    out
}
