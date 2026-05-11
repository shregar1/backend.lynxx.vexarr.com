use crate::error::GatewayError;
use crate::health::HealthRegistry;
use crate::metrics as gw_metrics;
use crate::plugins::PluginExecutor;
use crate::proxy::grpc::{handle_grpc, GrpcClient};
use crate::proxy::http_client::{HttpProxyClient, ProxyClientPool};
use crate::proxy::loadbalancer::{
    InFlightGuard, LbStrategy, LoadBalancerRegistry, RequestContext, STICKY_COOKIE_NAME,
};
use crate::proxy::router::SharedRouteTable;
use crate::proxy::websocket::handle_websocket;
use crate::quota::QuotaTracker;
use axum::body::Body;
use axum::extract::State;
use axum::http::{HeaderName, Request, Response, Uri};
use hyper_util::rt::TokioIo;
use std::sync::Arc;
use std::time::Instant;

/// Context attached to the request after the proxy matches a service+route
/// but before the plugin chain runs. Plugins (notably `request-recorder`)
/// pull this out of the request extensions to enrich their own captures
/// without re-doing the routing work.
#[derive(Clone, Debug)]
pub struct MatchContext {
    pub service_id: uuid::Uuid,
    pub route_id: uuid::Uuid,
    pub upstream_url: String,
    pub consumer_username: Option<String>,
}

#[derive(Clone)]
pub struct ProxyState {
    pub route_table: SharedRouteTable,
    pub plugins: Arc<PluginExecutor>,
    /// Parsed CIDR ranges for trusted proxy validation. When non-empty,
    /// X-Forwarded-For is only honoured from source IPs matching a range.
    pub trusted_proxies: Vec<ipnet::IpNet>,
    /// Streaming hyper-util client used for the default HTTP/1.1 + HTTP/2
    /// proxy hot path. Bodies forward without buffering. See
    /// `crate::proxy::http_client` for the rationale.
    ///
    /// For HTTPS upstreams that need bespoke TLS (mTLS, private CA, or
    /// `tls_verify = false`) the pool keeps per-service clients keyed
    /// by service id; the `default_client` here is used for everything
    /// else (the common case).
    pub http_client: HttpProxyClient,
    /// Per-service client pool. See [`ProxyClientPool`] for cache semantics.
    pub client_pool: Arc<ProxyClientPool>,
    pub grpc_client: GrpcClient,
    pub health: Arc<HealthRegistry>,
    pub lb: Arc<LoadBalancerRegistry>,
    pub quota: Arc<QuotaTracker>,
    /// Live licence state. The proxy hot path checks `is_active()`
    /// before serving — flipped off by the periodic re-verifier when
    /// the licence server reports the key is no longer active.
    pub license: crate::license::LicenseState,
    pub local_zone: Option<String>,
}

/// Hop-by-hop headers that must not be forwarded to the upstream. RFC 7230 §6.1.
/// `host` is also excluded so hyper synthesises it from the upstream URI.
const HOP_BY_HOP: [&str; 9] = [
    "host",
    "connection",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "te",
    "trailers",
    "transfer-encoding",
    "upgrade",
];

#[inline]
fn is_hop_by_hop(name: &HeaderName) -> bool {
    let s = name.as_str();
    HOP_BY_HOP.iter().any(|h| h.eq_ignore_ascii_case(s))
}

fn extract_cookie(headers: &axum::http::HeaderMap, name: &str) -> Option<String> {
    headers
        .get_all(axum::http::header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|s| s.split(';'))
        .find_map(|pair| {
            let pair = pair.trim();
            let (key, val) = pair.split_once('=')?;
            if key.trim() == name {
                Some(val.trim().to_string())
            } else {
                None
            }
        })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProtocolKind {
    WebSocket,
    Grpc,
    GrpcWeb,
    Http,
}

fn detect_protocol(req: &Request<Body>) -> ProtocolKind {
    let is_websocket = req
        .headers()
        .get(axum::http::header::UPGRADE)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.eq_ignore_ascii_case("websocket"))
        .unwrap_or(false);
    if is_websocket {
        return ProtocolKind::WebSocket;
    }

    let content_type = req
        .headers()
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    if content_type.starts_with("application/grpc-web") {
        return ProtocolKind::GrpcWeb;
    }
    if content_type.starts_with("application/grpc") {
        return ProtocolKind::Grpc;
    }

    ProtocolKind::Http
}

/// Parse a `unix:///path/to/socket` URL into `(socket_path, http_path)`.
/// Format: `unix:///var/run/app.sock` or `unix:///var/run/app.sock:/custom/path`
/// The part after the colon-separated path is the HTTP request path.
fn parse_unix_url(url: &str) -> Option<(String, String)> {
    let rest = url.strip_prefix("unix://")?;
    if let Some((sock, path)) = rest.split_once(':') {
        Some((sock.to_string(), path.to_string()))
    } else {
        Some((rest.to_string(), "/".to_string()))
    }
}

/// Proxy a request over a Unix domain socket using raw hyper HTTP/1.1.
async fn proxy_via_unix(
    socket_path: &str,
    mut req: Request<Body>,
) -> Result<Response<Body>, GatewayError> {
    let stream = tokio::net::UnixStream::connect(socket_path)
        .await
        .map_err(|e| {
            GatewayError::UpstreamError(format!("unix connect {socket_path}: {e}"))
        })?;
    let io = TokioIo::new(stream);

    let (mut sender, conn) = hyper::client::conn::http1::handshake(io)
        .await
        .map_err(|e| {
            GatewayError::UpstreamError(format!("unix handshake {socket_path}: {e}"))
        })?;

    tokio::spawn(async move {
        if let Err(e) = conn.await {
            tracing::debug!(error = %e, "unix upstream connection closed");
        }
    });

    // Ensure the request has a Host header (required by HTTP/1.1).
    if !req.headers().contains_key(axum::http::header::HOST) {
        if let Ok(v) = axum::http::HeaderValue::from_str("localhost") {
            req.headers_mut().insert(axum::http::header::HOST, v);
        }
    }

    let resp = sender.send_request(req).await.map_err(|e| {
        GatewayError::UpstreamError(format!("unix upstream send: {e}"))
    })?;

    let (parts, incoming) = resp.into_parts();
    Ok(Response::from_parts(parts, Body::new(incoming)))
}

pub async fn proxy_handler(
    State(state): State<ProxyState>,
    mut req: Request<Body>,
) -> Result<Response<Body>, GatewayError> {
    let start = Instant::now();
    let path = req.uri().path().to_string();
    let method = req.method().as_str().to_string();
    // HTTP/1.1 sends the hostname in the `Host` header; HTTP/2 uses the
    // `:authority` pseudo-header which hyper maps into `req.uri().host()`.
    // Check both so route matching works regardless of protocol version.
    let host = req
        .headers()
        .get(axum::http::header::HOST)
        .and_then(|v| v.to_str().ok())
        .map(|h| h.split(':').next().unwrap_or(h).to_string())
        .or_else(|| req.uri().host().map(|h| h.to_string()));

    let query_string = req.uri().query().map(|q| q.to_string());
    let (route, service) = match state
        .route_table
        .match_route_full(
            &path,
            &method,
            host.as_deref(),
            Some(req.headers()),
            query_string.as_deref(),
        )
    {
        Some(pair) => pair,
        None => {
            metrics::counter!(
                "lynxx_requests_total",
                "service" => "-",
                "route" => "-",
                "method" => method.clone(),
                "status" => "no_route",
                "target" => "-",
            )
            .increment(1);
            return Err(GatewayError::RouteNotFound(path));
        }
    };

    let svc_name = service.name.clone();
    let route_id = route.id.to_string();

    let open_duration_ms = state.health.config().open_duration_ms;
    if !state
        .health
        .should_allow_request(&service.id, open_duration_ms)
    {
        metrics::counter!(
            "lynxx_requests_total",
            "service" => svc_name.clone(),
            "route" => route_id.clone(),
            "method" => method.clone(),
            "status" => "circuit_open",
            "target" => "-",
        )
        .increment(1);
        return Err(GatewayError::CircuitOpen(format!(
            "service {} circuit breaker open",
            service.name
        )));
    }

    // Licence kill-switch — flipped off by the background re-verifier
    // when the licence server reports the key is cancelled / invalid.
    // Returns 402 Payment Required so customers see a clear billing
    // signal rather than a generic 5xx.
    if !state.license.is_active() {
        metrics::counter!(
            "lynxx_requests_total",
            "service" => svc_name.clone(),
            "route" => route_id.clone(),
            "method" => method.clone(),
            "status" => "license_revoked",
            "target" => "-",
        )
        .increment(1);
        return Err(GatewayError::LicenseRevoked);
    }

    // Quota check — counts the call and rejects with 429 if the monthly limit
    // is exhausted. Unlimited plans (Business, Enterprise) always pass.
    if !state.quota.check_and_count() {
        metrics::counter!(
            "lynxx_requests_total",
            "service" => svc_name.clone(),
            "route" => route_id.clone(),
            "method" => method.clone(),
            "status" => "quota_exceeded",
            "target" => "-",
        )
        .increment(1);
        return Err(GatewayError::QuotaExceeded);
    }

    // Extract client IP, respecting trusted_proxies CIDR allowlist.
    // When trusted_proxies is configured, only honour X-Forwarded-For
    // from source addresses that match the allowlist.
    let peer_ip = req
        .extensions()
        .get::<std::net::SocketAddr>()
        .map(|a| a.ip());
    let trust_headers = if state.trusted_proxies.is_empty() {
        true // no allowlist → trust unconditionally (backward compat)
    } else {
        peer_ip
            .map(|ip| state.trusted_proxies.iter().any(|net| net.contains(&ip)))
            .unwrap_or(false)
    };
    let client_ip_owned = if trust_headers {
        req.headers()
            .get("x-real-ip")
            .or_else(|| req.headers().get("x-forwarded-for"))
            .and_then(|v| v.to_str().ok())
            .map(|s| s.split(',').next().unwrap_or(s).trim().to_string())
            .unwrap_or_else(|| peer_ip.map(|ip| ip.to_string()).unwrap_or_default())
    } else {
        peer_ip.map(|ip| ip.to_string()).unwrap_or_default()
    };
    let sticky_cookie_owned = extract_cookie(req.headers(), STICKY_COOKIE_NAME);
    let lb_ctx = RequestContext {
        client_ip: Some(client_ip_owned.as_str()),
        uri_path: Some(path.as_str()),
        sticky_cookie: sticky_cookie_owned.as_deref(),
    };

    // Pick a target via load balancer if one is configured for this service.
    // Falls back to `service.url` if no targets exist.
    let picked = state.lb.pick(&service.id, state.local_zone.as_deref(), &lb_ctx);
    let picked_target_id = picked.as_ref().map(|ts| ts.target.id);
    let (upstream_base, target_label, _in_flight) = match &picked {
        Some(ts) => {
            let guard = InFlightGuard::new(ts.clone());
            (ts.target.url.clone(), ts.target.url.clone(), Some(guard))
        }
        None => (service.url.clone(), "-".to_string(), None),
    };

    // Compute upstream_url up-front so we can attach it to the MatchContext
    // before plugins run; the recorder needs it to populate `Recording.upstream_url`.
    let upstream_path = state.route_table.resolve_upstream_path(&route, &path);
    let query = req.uri().query().map(|q| q.to_string());
    let trimmed_base = upstream_base.trim_end_matches('/');
    let mut upstream_url = String::with_capacity(
        trimmed_base.len() + upstream_path.len() + query.as_ref().map(|q| q.len() + 1).unwrap_or(0),
    );
    upstream_url.push_str(trimmed_base);
    upstream_url.push_str(&upstream_path);
    if let Some(q) = &query {
        upstream_url.push('?');
        upstream_url.push_str(q);
    }

    // Stash routing context on the request so plugins (e.g. request-recorder)
    // can read service_id / route_id / upstream_url without re-matching.
    let consumer_username = req
        .headers()
        .get("x-consumer-username")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    req.extensions_mut().insert(MatchContext {
        service_id: service.id,
        route_id: route.id,
        upstream_url: upstream_url.clone(),
        consumer_username,
    });

    state
        .plugins
        .run_request(&mut req, Some(service.id), Some(route.id))
        .await
        .map_err(GatewayError::PluginError)?;

    // ── Static file serving: short-circuit the proxy path ──
    if service.service_type == "static" {
        if let Some(ref root) = service.document_root {
            let accept_enc = req
                .headers()
                .get(axum::http::header::ACCEPT_ENCODING)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("");
            let if_none_match = req
                .headers()
                .get(axum::http::header::IF_NONE_MATCH)
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string());
            let if_modified_since = req
                .headers()
                .get(axum::http::header::IF_MODIFIED_SINCE)
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string());
            let range_header = req
                .headers()
                .get(axum::http::header::RANGE)
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string());
            let serve_opts = crate::proxy::static_files::ServeOptions {
                hide_patterns: &[],
                cache_control: None,
                if_none_match: if_none_match.as_deref(),
                if_modified_since: if_modified_since.as_deref(),
                range: range_header.as_deref(),
            };
            let mut file_resp = crate::proxy::static_files::serve_with_options(
                root,
                &service.index_file,
                service.spa_fallback,
                &path,
                accept_enc,
                &serve_opts,
            )
            .await;

            // Transfer request extensions so script-runner's on_response
            // can access the ScriptRequestContext stashed during on_request.
            if let Some(ctx) = req.extensions().get::<crate::plugins::script_runner::ScriptRequestContext>() {
                file_resp.extensions_mut().insert(ctx.clone());
            }

            state
                .plugins
                .run_response(&mut file_resp, Some(service.id), Some(route.id))
                .await
                .map_err(GatewayError::PluginError)?;

            let bucket = gw_metrics::status_bucket(file_resp.status().as_u16());
            metrics::counter!(
                "lynxx_requests_total",
                "service" => svc_name.clone(),
                "route" => route_id.clone(),
                "method" => method.clone(),
                "status" => bucket,
                "target" => "static",
            )
            .increment(1);
            metrics::histogram!(
                "lynxx_request_duration_seconds",
                "service" => svc_name,
                "route" => route_id,
                "method" => method,
                "target" => "static",
            )
            .record(start.elapsed().as_secs_f64());

            return Ok(file_resp);
        }
    }

    // Dispatch on protocol. WebSocket and gRPC need streaming flows that the
    // reqwest-based path below cannot provide.
    match detect_protocol(&req) {
        ProtocolKind::WebSocket => {
            let result = handle_websocket(req, upstream_url).await;
            let status_label = match &result {
                Ok(r) => gw_metrics::status_bucket(r.status().as_u16()),
                Err(_) => "5xx",
            };
            metrics::counter!(
                "lynxx_requests_total",
                "service" => svc_name.clone(),
                "route" => route_id.clone(),
                "method" => method.clone(),
                "status" => status_label,
                "target" => target_label.clone(),
            )
            .increment(1);
            metrics::histogram!(
                "lynxx_request_duration_seconds",
                "service" => svc_name,
                "route" => route_id,
                "method" => method,
                "target" => target_label,
            )
            .record(start.elapsed().as_secs_f64());
            return result;
        }
        ProtocolKind::GrpcWeb => {
            let result =
                crate::proxy::grpc::handle_grpc_web(&state.grpc_client, req, upstream_url).await;
            let status_label = match &result {
                Ok(r) => gw_metrics::status_bucket(r.status().as_u16()),
                Err(_) => "5xx",
            };
            match &result {
                Ok(_) => state.health.record_success(&service.id, &svc_name),
                Err(e) => state.health.record_failure(
                    &service.id,
                    &svc_name,
                    format!("grpc-web upstream error: {e}"),
                ),
            }
            metrics::counter!(
                "lynxx_requests_total",
                "service" => svc_name.clone(),
                "route" => route_id.clone(),
                "method" => method.clone(),
                "status" => status_label,
                "target" => target_label.clone(),
            )
            .increment(1);
            metrics::histogram!(
                "lynxx_request_duration_seconds",
                "service" => svc_name,
                "route" => route_id,
                "method" => method,
                "target" => target_label,
            )
            .record(start.elapsed().as_secs_f64());
            return result;
        }
        ProtocolKind::Grpc => {
            let result = handle_grpc(&state.grpc_client, req, upstream_url).await;
            let status_label = match &result {
                Ok(r) => gw_metrics::status_bucket(r.status().as_u16()),
                Err(_) => "5xx",
            };
            match &result {
                Ok(_) => state.health.record_success(&service.id, &svc_name),
                Err(e) => state.health.record_failure(
                    &service.id,
                    &svc_name,
                    format!("grpc upstream error: {e}"),
                ),
            }
            metrics::counter!(
                "lynxx_requests_total",
                "service" => svc_name.clone(),
                "route" => route_id.clone(),
                "method" => method.clone(),
                "status" => status_label,
                "target" => target_label.clone(),
            )
            .increment(1);
            metrics::histogram!(
                "lynxx_request_duration_seconds",
                "service" => svc_name,
                "route" => route_id,
                "method" => method,
                "target" => target_label,
            )
            .record(start.elapsed().as_secs_f64());
            return result;
        }
        ProtocolKind::Http => {}
    }

    // ---- Unix socket upstream: bypass hyper-util, use raw HTTP/1.1. -------
    if upstream_url.starts_with("unix://") {
        if let Some((sock_path, http_path)) = parse_unix_url(&upstream_url) {
            let cache_key = req
                .headers()
                .get(crate::plugins::proxy_cache::CACHE_KEY_HEADER)
                .cloned();
            let recorder_id = req.headers().get("x-lynxx-recorder-id").cloned();

            let (mut parts, body) = req.into_parts();

            let unix_uri: Uri = http_path
                .parse()
                .unwrap_or_else(|_| Uri::from_static("/"));
            parts.uri = unix_uri;

            let extensions =
                std::mem::replace(&mut parts.extensions, axum::http::Extensions::default());

            parts
                .headers
                .remove(crate::plugins::proxy_cache::CACHE_KEY_HEADER);
            parts.headers.remove("x-lynxx-recorder-id");
            for h in HOP_BY_HOP.iter() {
                if let Ok(name) = HeaderName::from_bytes(h.as_bytes()) {
                    parts.headers.remove(&name);
                }
            }
            parts.version = axum::http::Version::HTTP_11;

            let unix_req = Request::from_parts(parts, body);
            let unix_resp = match proxy_via_unix(&sock_path, unix_req).await {
                Ok(r) => r,
                Err(e) => {
                    state.health.record_failure(
                        &service.id,
                        &svc_name,
                        format!("unix socket upstream: {e}"),
                    );
                    metrics::counter!(
                        "lynxx_requests_total",
                        "service" => svc_name.clone(),
                        "route" => route_id.clone(),
                        "method" => method.clone(),
                        "status" => "5xx",
                        "target" => target_label.clone(),
                    )
                    .increment(1);
                    return Err(e);
                }
            };

            let (resp_parts, incoming) = unix_resp.into_parts();
            let status = resp_parts.status;
            let resp_body = match axum::body::to_bytes(Body::new(incoming), usize::MAX).await {
                Ok(b) => b,
                Err(e) => {
                    return Err(GatewayError::UpstreamError(format!(
                        "unix body read: {e}"
                    )));
                }
            };

            if status.is_server_error() {
                state
                    .health
                    .record_failure(&service.id, &svc_name, format!("upstream {status}"));
            } else {
                state.health.record_success(&service.id, &svc_name);
            }

            let mut plugin_resp = Response::new(Body::from(resp_body.clone()));
            *plugin_resp.status_mut() = status;
            *plugin_resp.headers_mut() = resp_parts.headers;
            *plugin_resp.extensions_mut() = extensions;
            if let Some(v) = cache_key {
                plugin_resp
                    .headers_mut()
                    .insert(crate::plugins::proxy_cache::CACHE_KEY_HEADER, v);
            }
            if let Some(v) = recorder_id {
                plugin_resp
                    .headers_mut()
                    .insert("x-lynxx-recorder-id", v);
            }

            state
                .plugins
                .run_response(&mut plugin_resp, Some(service.id), Some(route.id))
                .await
                .map_err(GatewayError::PluginError)?;

            let (final_parts, _) = plugin_resp.into_parts();
            let mut final_resp = Response::new(Body::from(resp_body));
            *final_resp.status_mut() = final_parts.status;
            *final_resp.headers_mut() = final_parts.headers;

            let bucket = gw_metrics::status_bucket(status.as_u16());
            metrics::counter!(
                "lynxx_requests_total",
                "service" => svc_name.clone(),
                "route" => route_id.clone(),
                "method" => method.clone(),
                "status" => bucket,
                "target" => target_label.clone(),
            )
            .increment(1);
            metrics::histogram!(
                "lynxx_request_duration_seconds",
                "service" => svc_name,
                "route" => route_id,
                "method" => method,
                "target" => target_label,
            )
            .record(start.elapsed().as_secs_f64());

            return Ok(final_resp);
        }
    }

    // ---- Hot path: stream the body straight through hyper-util. -----------
    //
    // OPTIMIZATION: this branch deliberately avoids three sources of overhead
    // that the previous reqwest-based path paid on every request:
    //   1. body buffering (`axum::body::to_bytes`) — replaced by streaming
    //      `axum::body::Body` directly into the upstream request,
    //   2. `HeaderMap` clone + per-header `to_str` — we move the inbound
    //      headers into the upstream request via `into_parts`, only patching
    //      the few we must change (URI, hop-by-hop strip, optional cache key),
    //   3. URL re-parsing inside reqwest — we parse once into `hyper::Uri`.
    //
    // Body buffering is still necessary on the *response* side because the
    // plugin chain (`on_response`) operates on a fully-materialised
    // `Response<BoxBody>`. With an empty plugin chain the fast-path in
    // `PluginExecutor` skips that work; we still buffer once to satisfy the
    // type, but no heap copy beyond that.
    //
    // Expected win: ~80-150µs at p50 in synthetic benchmarks against
    // a tiny echo upstream, with a larger win at p99 due to fewer allocator hits.
    let upstream_uri: Uri = upstream_url
        .parse()
        .map_err(|e| GatewayError::BadRequest(format!("invalid upstream uri: {e}")))?;

    // Capture the proxy-cache key (set by the request-side plugin) before we
    // strip it; we need to re-attach it to the response for `on_response`.
    let cache_key = req
        .headers()
        .get(crate::plugins::proxy_cache::CACHE_KEY_HEADER)
        .cloned();
    // Same bridge for the request-recorder's capture id. The plugin sets a
    // request header in on_request and reads it back in on_response.
    let recorder_id = req.headers().get("x-lynxx-recorder-id").cloned();

    // Move parts out so we don't clone the HeaderMap. `req` is consumed.
    let (mut parts, body) = req.into_parts();
    parts.uri = upstream_uri;
    
    // Capture extensions before they are lost so we can re-attach them to 
    // the response for the plugin chain.
    let extensions = std::mem::replace(&mut parts.extensions, axum::http::Extensions::default());

    // Strip hop-by-hop and our internal proxy-cache marker.
    parts
        .headers
        .remove(crate::plugins::proxy_cache::CACHE_KEY_HEADER);
    parts.headers.remove("x-lynxx-recorder-id");
    for h in HOP_BY_HOP.iter() {
        // safety: the constants are valid header names.
        if let Ok(name) = HeaderName::from_bytes(h.as_bytes()) {
            parts.headers.remove(&name);
        }
    }
    // Force HTTP/1.1 over the wire — the connector negotiates h2 via ALPN
    // for https. For http:// we use HTTP/1.1 which is what most
    // upstreams expect from a forward proxy.
    parts.version = axum::http::Version::HTTP_11;

    // Per-service client lookup. For plain HTTP and for HTTPS services that
    // don't customise TLS (the common case) this resolves to the shared
    // default client without a hashmap miss in the steady state.
    let client = match state.client_pool.client_for(&service) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(
                service = %svc_name,
                error = %e,
                "failed to build per-service tls client; falling back to default"
            );
            state.http_client.clone()
        }
    };

    let max_attempts = (service.retries + 1).max(1) as usize;
    let retry_on_status = &route.retry_on_status;
    // Force buffering when route.buffer_request is set, or when retries > 0,
    // or when retry_on_status is configured (need to re-read body on retry).
    let force_buffer = route.buffer_request || !retry_on_status.is_empty();
    let mut last_err = String::new();
    let mut upstream_resp = None;

    if max_attempts <= 1 && !force_buffer {
        let upstream_req = Request::from_parts(parts, body);
        match client.request(upstream_req).await {
            Ok(r) => upstream_resp = Some(r),
            Err(e) => {
                let kind = if e.is_connect() { "connect" } else { "other" };
                metrics::counter!(
                    "lynxx_upstream_errors_total",
                    "service" => svc_name.clone(),
                    "kind" => kind,
                )
                .increment(1);
                last_err = e.to_string();
            }
        }
    } else {
        let body_bytes = axum::body::to_bytes(body, 10 * 1024 * 1024)
            .await
            .map_err(|e| GatewayError::BadRequest(format!("failed to buffer request body: {e}")))?;

        for attempt in 0..max_attempts {
            if attempt > 0 {
                metrics::counter!(
                    "lynxx_upstream_retries_total",
                    "service" => svc_name.clone(),
                )
                .increment(1);
                tokio::time::sleep(std::time::Duration::from_millis(50 * (1 << attempt.min(4))))
                    .await;
            }

            let retry_body = Body::from(body_bytes.clone());
            let upstream_req = Request::from_parts(parts.clone(), retry_body);
            match client.request(upstream_req).await {
                Ok(r) => {
                    // Check if we should retry based on status code.
                    if !retry_on_status.is_empty()
                        && retry_on_status.contains(&r.status().as_u16())
                        && attempt + 1 < max_attempts
                    {
                        last_err = format!("upstream returned {}", r.status());
                        metrics::counter!(
                            "lynxx_upstream_retries_total",
                            "service" => svc_name.clone(),
                        )
                        .increment(1);
                        continue;
                    }
                    upstream_resp = Some(r);
                    break;
                }
                Err(e) => {
                    let kind = if e.is_connect() { "connect" } else { "other" };
                    metrics::counter!(
                        "lynxx_upstream_errors_total",
                        "service" => svc_name.clone(),
                        "kind" => kind,
                    )
                    .increment(1);
                    last_err = e.to_string();
                    if !e.is_connect() {
                        break;
                    }
                }
            }
        }
    }

    let upstream_resp = match upstream_resp {
        Some(r) => r,
        None => {
            metrics::counter!(
                "lynxx_requests_total",
                "service" => svc_name.clone(),
                "route" => route_id.clone(),
                "method" => method.clone(),
                "status" => "5xx",
                "target" => target_label.clone(),
            )
            .increment(1);
            metrics::histogram!(
                "lynxx_request_duration_seconds",
                "service" => svc_name.clone(),
                "route" => route_id.clone(),
                "method" => method.clone(),
                "target" => target_label.clone(),
            )
            .record(start.elapsed().as_secs_f64());
            state.health.record_failure(
                &service.id,
                &svc_name,
                format!("upstream send error: {last_err}"),
            );
            return Err(GatewayError::UpstreamError(last_err));
        }
    };

    let (resp_parts, incoming) = upstream_resp.into_parts();
    let status = resp_parts.status;

    // Buffer the response body once so the plugin chain can mutate it. With
    // the empty-chain fast path this stays cheap — only one heap allocation.
    let resp_body = match axum::body::to_bytes(Body::new(incoming), usize::MAX).await {
        Ok(b) => b,
        Err(e) => {
            metrics::counter!(
                "lynxx_upstream_errors_total",
                "service" => svc_name.clone(),
                "kind" => "other",
            )
            .increment(1);
            metrics::counter!(
                "lynxx_requests_total",
                "service" => svc_name.clone(),
                "route" => route_id.clone(),
                "method" => method.clone(),
                "status" => "5xx",
                "target" => target_label.clone(),
            )
            .increment(1);
            metrics::histogram!(
                "lynxx_request_duration_seconds",
                "service" => svc_name.clone(),
                "route" => route_id.clone(),
                "method" => method.clone(),
                "target" => target_label.clone(),
            )
            .record(start.elapsed().as_secs_f64());
            state.health.record_failure(
                &service.id,
                &svc_name,
                format!("upstream body error: {e}"),
            );
            return Err(GatewayError::UpstreamError(e.to_string()));
        }
    };

    if status.is_server_error() {
        metrics::counter!(
            "lynxx_upstream_errors_total",
            "service" => svc_name.clone(),
            "kind" => "5xx",
        )
        .increment(1);
        state
            .health
            .record_failure(&service.id, &svc_name, format!("upstream {status}"));
    } else {
        state.health.record_success(&service.id, &svc_name);
    }

    // Build the plugin-facing response. We reuse the upstream HeaderMap by
    // moving it through `Response::from_parts` — no clone.
    let mut plugin_resp = Response::new(Body::from(resp_body.clone()));
    *plugin_resp.status_mut() = status;
    *plugin_resp.headers_mut() = resp_parts.headers;
    *plugin_resp.extensions_mut() = extensions;

    if let Some(v) = cache_key {
        plugin_resp
            .headers_mut()
            .insert(crate::plugins::proxy_cache::CACHE_KEY_HEADER, v);
    }
    if let Some(v) = recorder_id {
        plugin_resp.headers_mut().insert("x-lynxx-recorder-id", v);
    }

    state
        .plugins
        .run_response(&mut plugin_resp, Some(service.id), Some(route.id))
        .await
        .map_err(GatewayError::PluginError)?;

    let (final_parts, _bb) = plugin_resp.into_parts();
    let mut final_resp = Response::new(Body::from(resp_body));
    *final_resp.status_mut() = final_parts.status;
    *final_resp.headers_mut() = final_parts.headers;

    // Set sticky-session cookie so subsequent requests from this client
    // land on the same target.
    if matches!(
        LbStrategy::from_str(&service.lb_strategy),
        LbStrategy::CookieSticky
    ) {
        if let Some(tid) = picked_target_id {
            if let Ok(val) = format!(
                "{}={}; Path=/; HttpOnly; SameSite=Lax",
                STICKY_COOKIE_NAME, tid
            )
            .parse()
            {
                final_resp
                    .headers_mut()
                    .append(axum::http::header::SET_COOKIE, val);
            }
        }
    }

    let bucket = gw_metrics::status_bucket(status.as_u16());
    metrics::counter!(
        "lynxx_requests_total",
        "service" => svc_name.clone(),
        "route" => route_id.clone(),
        "method" => method.clone(),
        "status" => bucket,
        "target" => target_label.clone(),
    )
    .increment(1);
    metrics::histogram!(
        "lynxx_request_duration_seconds",
        "service" => svc_name,
        "route" => route_id,
        "method" => method,
        "target" => target_label,
    )
    .record(start.elapsed().as_secs_f64());

    Ok(final_resp)
}
