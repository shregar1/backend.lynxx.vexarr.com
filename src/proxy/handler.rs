use crate::error::GatewayError;
use crate::plugins::{BoxBody, PluginRegistry};
use crate::proxy::router::SharedRouteTable;
use axum::body::Body;
use axum::extract::State;
use axum::http::{Request, Response};
use std::sync::Arc;

#[derive(Clone)]
pub struct ProxyState {
    pub route_table: SharedRouteTable,
    pub plugins: Arc<PluginRegistry>,
    pub http_client: reqwest::Client,
}

pub async fn proxy_handler(
    State(state): State<ProxyState>,
    mut req: Request<Body>,
) -> Result<Response<Body>, GatewayError> {
    let path = req.uri().path().to_string();
    let method = req.method().as_str().to_string();

    let (route, service) = state
        .route_table
        .match_route(&path, &method)
        .ok_or_else(|| GatewayError::RouteNotFound(path.clone()))?;

    for plugin in state.plugins.plugins() {
        plugin
            .on_request(&mut req)
            .await
            .map_err(GatewayError::PluginError)?;
    }

    let upstream_path = state.route_table.resolve_upstream_path(&route, &path);
    let query = req.uri().query().map(|q| format!("?{q}")).unwrap_or_default();
    let upstream_url = format!("{}{upstream_path}{query}", service.url);

    let headers = req.headers().clone();
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
        if key == "host" || key == "connection" {
            continue;
        }
        if let Ok(v) = value.to_str() {
            upstream_req = upstream_req.header(key.as_str(), v);
        }
    }

    let upstream_resp = upstream_req
        .send()
        .await
        .map_err(|e| GatewayError::UpstreamError(e.to_string()))?;

    let status = upstream_resp.status();
    let resp_headers = upstream_resp.headers().clone();
    let resp_body = upstream_resp
        .bytes()
        .await
        .map_err(|e| GatewayError::UpstreamError(e.to_string()))?;

    let mut plugin_resp = Response::builder()
        .status(status.as_u16())
        .body(BoxBody::new(resp_body.clone()))
        .map_err(|e| GatewayError::Internal(e.to_string()))?;

    for (key, value) in &resp_headers {
        plugin_resp.headers_mut().insert(key, value.clone());
    }

    for plugin in state.plugins.plugins() {
        plugin
            .on_response(&mut plugin_resp)
            .await
            .map_err(GatewayError::PluginError)?;
    }

    let mut final_resp = Response::builder()
        .status(plugin_resp.status())
        .body(Body::from(resp_body))
        .map_err(|e| GatewayError::Internal(e.to_string()))?;

    *final_resp.headers_mut() = plugin_resp.headers().clone();

    Ok(final_resp)
}
