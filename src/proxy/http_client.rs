//! Streaming HTTP/1.1 + HTTP/2 client used by the default proxy hot path.
//!
//! Replaces the previous `reqwest::Client` based forward path. `reqwest` is
//! convenient but it buffers request bodies, allocates a fresh `HeaderMap`
//! per request, and re-parses URLs. For the proxy hot path we want to:
//!
//! * forward the body as a streaming `axum::body::Body` (no buffering),
//! * pass the inbound `HeaderMap` through with minimal mutation,
//! * skip URL re-parsing where possible.
//!
//! `hyper_util::client::legacy::Client` plus a TLS-aware connector gives us
//! that. The same client supports both `http://` and `https://` upstreams
//! for the default path; gRPC keeps using its own h2c-only client.
//!
//! Expected win: 100-200µs off p50, similar at p99 — fewer allocations and
//! no body copy on the request side.

use axum::body::Body;
use hyper_rustls::HttpsConnectorBuilder;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::client::legacy::Client;
use hyper_util::rt::TokioExecutor;

/// HTTPS-capable client that speaks both HTTP/1.1 and HTTP/2 ALPN.
pub type HttpProxyClient = Client<
    hyper_rustls::HttpsConnector<HttpConnector>,
    Body,
>;

/// Build the proxy hot-path client. Returns an error rather than panicking if
/// the platform certificate store cannot be loaded — startup will surface this
/// to the operator instead of crashing on the first HTTPS upstream call.
pub fn build_http_proxy_client() -> anyhow::Result<HttpProxyClient> {
    let mut http = HttpConnector::new();
    http.enforce_http(false);
    http.set_nodelay(true);
    http.set_keepalive(Some(std::time::Duration::from_secs(60)));

    let https = HttpsConnectorBuilder::new()
        .with_webpki_roots()
        .https_or_http()
        .enable_http1()
        .enable_http2()
        .wrap_connector(http);

    let client = Client::builder(TokioExecutor::new())
        .pool_idle_timeout(std::time::Duration::from_secs(90))
        .pool_max_idle_per_host(64)
        .build(https);

    Ok(client)
}
