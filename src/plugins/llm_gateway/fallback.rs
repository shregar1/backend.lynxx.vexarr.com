//! Multi-provider dispatcher with fallback.
//!
//! `dispatch()` walks the ordered list returned by [`Router::dispatch_order`]
//! and POSTs the request body to each `{provider.base_url}{path}` until one
//! responds with a non-retriable status. Retriable = 429 or 5xx. All other
//! statuses (including client errors like 400/401) short-circuit and return
//! immediately — those are deterministic upstream signals and retrying would
//! just amplify the failure mode.
//!
//! The dispatcher does not know about token counting, caching, or guards;
//! those are applied by the caller before/after this layer. Its only job is
//! "send to upstream, retry on transient failure, count fallbacks".

use super::config::ProviderCfg;
use reqwest::Client;
use std::time::Duration;

/// Outcome of a single dispatch.
pub struct DispatchResult {
    pub provider: String,
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    /// Number of providers tried before this one succeeded (or failed
    /// non-retriably). Zero means the primary worked first time.
    pub fallback_count: u32,
    /// Set when every provider in the chain returned a retriable error;
    /// the body is from the *last* attempt.
    pub exhausted: bool,
}

/// Whether this status indicates "try the next provider".
fn is_retriable(status: u16) -> bool {
    status == 429 || (500..600).contains(&status)
}

pub async fn dispatch(
    client: &Client,
    providers: &[ProviderCfg],
    path: &str,
    method: &reqwest::Method,
    forwarded_headers: &[(String, String)],
    body: &[u8],
    on_fallback: impl Fn(&str, &str),
) -> Result<DispatchResult, String> {
    if providers.is_empty() {
        return Err("llm-gateway: no providers in dispatch chain".into());
    }
    let mut last: Option<DispatchResult> = None;
    for (i, p) in providers.iter().enumerate() {
        if i > 0 {
            on_fallback(&providers[i - 1].name, &p.name);
        }
        // Build the URL by joining base_url + path. We deliberately don't try
        // to be clever about double slashes — operators can fix their config
        // if there's a doubled `/`. trim is the one normalization we do.
        let base = p.base_url.trim_end_matches('/');
        let url = format!("{base}{path}");

        let mut req = client.request(method.clone(), &url);
        for (k, v) in forwarded_headers {
            // Skip auth headers from the client — we'll inject the provider's
            // own credential below.
            if k.eq_ignore_ascii_case("authorization") || k.eq_ignore_ascii_case("x-api-key") {
                continue;
            }
            // Skip hop-by-hop and length headers; reqwest sets these.
            let lk = k.to_ascii_lowercase();
            if matches!(
                lk.as_str(),
                "host"
                    | "content-length"
                    | "transfer-encoding"
                    | "connection"
                    | "keep-alive"
                    | "proxy-authenticate"
                    | "proxy-authorization"
                    | "te"
                    | "trailers"
                    | "upgrade"
                    | "accept-encoding"
            ) {
                continue;
            }
            req = req.header(k, v);
        }
        if let Some(key) = &p.api_key {
            let value = if p.auth_scheme.is_empty() {
                key.clone()
            } else {
                format!("{} {}", p.auth_scheme, key)
            };
            req = req.header(&p.auth_header, value);
        }
        // Anthropic also requires the version header. Set it if missing —
        // operators authoring with /v1/messages will appreciate the default.
        if p.base_url.contains("anthropic") {
            req = req.header("anthropic-version", "2023-06-01");
        }
        req = req.body(body.to_vec()).timeout(Duration::from_secs(120));

        let resp = match req.send().await {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(provider = %p.name, error = %e, "llm-gateway: provider request failed; trying next");
                last = Some(DispatchResult {
                    provider: p.name.clone(),
                    status: 502,
                    headers: vec![("Content-Type".into(), "application/json".into())],
                    body: format!(r#"{{"error":{{"code":"upstream_error","message":"{e}"}}}}"#)
                        .into_bytes(),
                    fallback_count: i as u32,
                    exhausted: false,
                });
                continue;
            }
        };
        let status = resp.status().as_u16();
        let mut headers = Vec::with_capacity(resp.headers().len());
        for (k, v) in resp.headers().iter() {
            if let Ok(s) = v.to_str() {
                headers.push((k.as_str().to_string(), s.to_string()));
            }
        }
        let body_bytes = match resp.bytes().await {
            Ok(b) => b.to_vec(),
            Err(e) => {
                tracing::warn!(provider = %p.name, error = %e, "llm-gateway: failed to read provider body");
                last = Some(DispatchResult {
                    provider: p.name.clone(),
                    status: 502,
                    headers,
                    body: format!(
                        r#"{{"error":{{"code":"upstream_body_error","message":"{e}"}}}}"#
                    )
                    .into_bytes(),
                    fallback_count: i as u32,
                    exhausted: false,
                });
                continue;
            }
        };

        if is_retriable(status) && i + 1 < providers.len() {
            tracing::info!(
                provider = %p.name,
                status,
                "llm-gateway: retriable upstream status, falling back"
            );
            last = Some(DispatchResult {
                provider: p.name.clone(),
                status,
                headers,
                body: body_bytes,
                fallback_count: i as u32,
                exhausted: false,
            });
            continue;
        }
        return Ok(DispatchResult {
            provider: p.name.clone(),
            status,
            headers,
            body: body_bytes,
            fallback_count: i as u32,
            exhausted: false,
        });
    }
    let mut last = last.expect("loop runs at least once");
    last.exhausted = true;
    Ok(last)
}
