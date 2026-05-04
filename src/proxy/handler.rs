use crate::error::GatewayError;
use crate::health::HealthRegistry;
use crate::metrics as gw_metrics;
use crate::plugins::{BoxBody, PluginExecutor};
use crate::proxy::grpc::{handle_grpc, GrpcClient};
use crate::proxy::loadbalancer::{InFlightGuard, LoadBalancerRegistry};
use crate::proxy::router::SharedRouteTable;
use crate::proxy::websocket::handle_websocket;
use axum::body::Body;
use axum::extract::State;
use axum::http::{Request, Response};
use std::sync::Arc;
use std::time::Instant;

#[derive(Clone)]
pub struct ProxyState {
    pub route_table: SharedRouteTable,
    pub plugins: Arc<PluginExecutor>,
    pub http_client: reqwest::Client,
    pub grpc_client: GrpcClient,
    pub health: Arc<HealthRegistry>,
    pub lb: Arc<LoadBalancerRegistry>,
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
    let query = req.uri().query().map(|q| format!("?{q}")).unwrap_or_default();
    let upstream_url = format!(
        "{}{upstream_path}{query}",
        upstream_base.trim_end_matches('/')
    );

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

    let headers = req.headers().clone();
    // Capture any request-side plugin annotations (e.g. proxy-cache key) we
    // need to forward onto the plugin response chain. These are stripped
    // from the upstream request below.
    let cache_key = headers
        .get(crate::plugins::proxy_cache::CACHE_KEY_HEADER)
        .cloned();
    let body_bytes = axum::body::to_bytes(req.into_body(), 10 * 1024 * 1024)
        .await
        .map_err(|e| GatewayError::Internal(format!("failed to read body: {e}")))?;

    let mut upstream_req = state
        .http_client
        .request(
            reqwest::Method::from_bytes(method.as_bytes())
                .map_err(|e| GatewayError::Internal(e.to_string()))?,
            &upstream_url,
        )
        .body(body_bytes.to_vec());

    for (key, value) in &headers {
        if key == "host"
            || key == "connection"
            || key.as_str() == crate::plugins::proxy_cache::CACHE_KEY_HEADER
        {
            continue;
        }
        if let Ok(v) = value.to_str() {
            upstream_req = upstream_req.header(key.as_str(), v);
        }
    }

    let upstream_resp = match upstream_req.send().await {
        Ok(r) => r,
        Err(e) => {
            let kind = if e.is_timeout() { "timeout" } else { "connect" };
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
            state
                .health
                .record_failure(&service.id, &svc_name, format!("upstream send error: {e}"));
            return Err(GatewayError::UpstreamError(e.to_string()));
        }
    };

    let status = upstream_resp.status();
    let resp_headers = upstream_resp.headers().clone();
    let resp_body = match upstream_resp.bytes().await {
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
            state
                .health
                .record_failure(&service.id, &svc_name, format!("upstream body error: {e}"));
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

    let mut plugin_resp = Response::builder()
        .status(status.as_u16())
        .body(BoxBody::new(resp_body.clone()))
        .map_err(|e| GatewayError::Internal(e.to_string()))?;

    for (key, value) in &resp_headers {
        plugin_resp.headers_mut().insert(key, value.clone());
    }
    // Forward the proxy-cache key from request to response so the cache
    // plugin's `on_response` can record under the correct key.
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

    let mut final_resp = Response::builder()
        .status(plugin_resp.status())
        .body(Body::from(resp_body))
        .map_err(|e| GatewayError::Internal(e.to_string()))?;

    *final_resp.headers_mut() = plugin_resp.headers().clone();

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
