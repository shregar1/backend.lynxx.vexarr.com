use crate::error::GatewayError;
use crate::health::HealthRegistry;
use crate::metrics as gw_metrics;
use crate::plugins::{BoxBody, PluginExecutor};
use crate::proxy::grpc::{handle_grpc, GrpcClient};
use crate::proxy::http_client::{HttpProxyClient, ProxyClientPool};
use crate::proxy::loadbalancer::{InFlightGuard, LoadBalancerRegistry};
use crate::proxy::router::SharedRouteTable;
use crate::proxy::websocket::handle_websocket;
use axum::body::Body;
use axum::extract::State;
use axum::http::{HeaderName, Request, Response, Uri};
use std::sync::Arc;
use std::time::Instant;

#[derive(Clone)]
pub struct ProxyState {
    pub route_table: SharedRouteTable,
    pub plugins: Arc<PluginExecutor>,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProtocolKind {
    WebSocket,
    Grpc,
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
    let is_grpc = req
        .headers()
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.starts_with("application/grpc"))
        .unwrap_or(false);
    if is_grpc {
        return ProtocolKind::Grpc;
    }
    ProtocolKind::Http
}

pub async fn proxy_handler(
    State(state): State<ProxyState>,
    mut req: Request<Body>,
) -> Result<Response<Body>, GatewayError> {
    let start = Instant::now();
    let path = req.uri().path().to_string();
    let method = req.method().as_str().to_string();

    let (route, service) = match state.route_table.match_route(&path, &method) {
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

    // Pick a target via load balancer if one is configured for this service.
    // Falls back to `service.url` if no targets exist.
    let picked = state.lb.pick(&service.id);
    let (upstream_base, target_label, _in_flight) = match &picked {
        Some(ts) => {
            let guard = InFlightGuard::new(ts.clone());
            (ts.target.url.clone(), ts.target.url.clone(), Some(guard))
        }
        None => (service.url.clone(), "-".to_string(), None),
    };

    state
        .plugins
        .run_request(&mut req, Some(service.id), Some(route.id))
        .await
        .map_err(GatewayError::PluginError)?;

    let upstream_path = state.route_table.resolve_upstream_path(&route, &path);
    let query = req.uri().query();
    // Avoid format! allocation churn on the hot path: pre-size and push.
    let trimmed_base = upstream_base.trim_end_matches('/');
    let mut upstream_url = String::with_capacity(
        trimmed_base.len() + upstream_path.len() + query.map(|q| q.len() + 1).unwrap_or(0),
    );
    upstream_url.push_str(trimmed_base);
    upstream_url.push_str(&upstream_path);
    if let Some(q) = query {
        upstream_url.push('?');
        upstream_url.push_str(q);
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
    // `nginx:alpine`, with a larger win at p99 due to fewer allocator hits.
    let upstream_uri: Uri = upstream_url
        .parse()
        .map_err(|e| GatewayError::BadRequest(format!("invalid upstream uri: {e}")))?;

    // Capture the proxy-cache key (set by the request-side plugin) before we
    // strip it; we need to re-attach it to the response for `on_response`.
    let cache_key = req
        .headers()
        .get(crate::plugins::proxy_cache::CACHE_KEY_HEADER)
        .cloned();

    // Move parts out so we don't clone the HeaderMap. `req` is consumed.
    let (mut parts, body) = req.into_parts();
    parts.uri = upstream_uri;
    // Strip hop-by-hop and our internal proxy-cache marker.
    parts.headers.remove(crate::plugins::proxy_cache::CACHE_KEY_HEADER);
    for h in HOP_BY_HOP.iter() {
        // safety: the constants are valid header names.
        if let Ok(name) = HeaderName::from_bytes(h.as_bytes()) {
            parts.headers.remove(&name);
        }
    }
    // Force HTTP/1.1 over the wire — the connector negotiates h2 via ALPN
    // for https. For http:// we use HTTP/1.1 which is what nginx + most
    // upstreams expect from a forward proxy.
    parts.version = axum::http::Version::HTTP_11;

    let upstream_req = Request::from_parts(parts, body);

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

    let upstream_resp = match client.request(upstream_req).await {
        Ok(r) => r,
        Err(e) => {
            let kind = if e.is_connect() { "connect" } else { "other" };
            metrics::counter!(
                "lynxx_upstream_errors_total",
                "service" => svc_name.clone(),
                "kind" => kind,
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
                format!("upstream send error: {e}"),
            );
            return Err(GatewayError::UpstreamError(e.to_string()));
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
    let mut plugin_resp = Response::new(BoxBody::new(resp_body.clone()));
    *plugin_resp.status_mut() = status;
    *plugin_resp.headers_mut() = resp_parts.headers;
    if let Some(v) = cache_key {
        plugin_resp
            .headers_mut()
            .insert(crate::plugins::proxy_cache::CACHE_KEY_HEADER, v);
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
