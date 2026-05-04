use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;

#[derive(Debug, thiserror::Error)]
pub enum GatewayError {
    #[error("route not found: {0}")]
    RouteNotFound(String),

    #[error("service not found: {0}")]
    ServiceNotFound(String),

    #[error("upstream error: {0}")]
    UpstreamError(String),

    #[error("circuit open: {0}")]
    CircuitOpen(String),

    #[error("plugin error: {0}")]
    PluginError(PluginError),

    #[error("config error: {0}")]
    ConfigError(String),

    #[error("not found: {0}")]
    NotFound(String),

    #[error("bad request: {0}")]
    BadRequest(String),

    #[error("internal error: {0}")]
    Internal(String),

    #[error("database error: {0}")]
    Sqlx(#[from] sqlx::Error),

    #[error("migration error: {0}")]
    Migrate(#[from] sqlx::migrate::MigrateError),
}

#[derive(Debug, thiserror::Error)]
pub enum PluginError {
    #[error("rate limited")]
    RateLimited,

    #[error("unauthorized: {0}")]
    Unauthorized(String),

    #[error("forbidden: {0}")]
    Forbidden(String),

    #[error("payload too large: {0}")]
    PayloadTooLarge(String),

    #[error("plugin internal error: {0}")]
    Internal(String),

    /// Returned by plugins (e.g. proxy-cache hit, request-termination) to
    /// short-circuit the request and return a synthesized response without
    /// contacting upstream. The handler maps this directly to a Response.
    #[error("short-circuit response")]
    ShortCircuit {
        status: u16,
        headers: Vec<(String, String)>,
        body: Vec<u8>,
    },
}

impl IntoResponse for GatewayError {
    fn into_response(self) -> Response {
        // ShortCircuit gets special handling: build a real Response with
        // the supplied status, headers, and body — no JSON envelope.
        if let GatewayError::PluginError(PluginError::ShortCircuit {
            status,
            headers,
            body,
        }) = &self
        {
            let status_code =
                StatusCode::from_u16(*status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
            let mut response = Response::new(axum::body::Body::from(body.clone()));
            *response.status_mut() = status_code;
            for (k, v) in headers {
                if let (Ok(name), Ok(value)) = (
                    axum::http::HeaderName::from_bytes(k.as_bytes()),
                    HeaderValue::from_str(v),
                ) {
                    response.headers_mut().append(name, value);
                }
            }
            return response;
        }

        let mut circuit_open = false;
        let (status, message) = match &self {
            GatewayError::RouteNotFound(_) => (StatusCode::NOT_FOUND, self.to_string()),
            GatewayError::ServiceNotFound(_) => (StatusCode::BAD_GATEWAY, self.to_string()),
            GatewayError::UpstreamError(_) => (StatusCode::BAD_GATEWAY, self.to_string()),
            GatewayError::CircuitOpen(_) => {
                circuit_open = true;
                (StatusCode::SERVICE_UNAVAILABLE, self.to_string())
            }
            GatewayError::PluginError(e) => match e {
                PluginError::RateLimited => {
                    (StatusCode::TOO_MANY_REQUESTS, "rate limit exceeded".into())
                }
                PluginError::Unauthorized(msg) => (StatusCode::UNAUTHORIZED, msg.clone()),
                PluginError::Forbidden(msg) => (StatusCode::FORBIDDEN, msg.clone()),
                PluginError::PayloadTooLarge(msg) => (StatusCode::PAYLOAD_TOO_LARGE, msg.clone()),
                PluginError::Internal(msg) => (StatusCode::INTERNAL_SERVER_ERROR, msg.clone()),
                PluginError::ShortCircuit { .. } => unreachable!("handled above"),
            },
            GatewayError::NotFound(_) => (StatusCode::NOT_FOUND, self.to_string()),
            GatewayError::BadRequest(_) => (StatusCode::BAD_REQUEST, self.to_string()),
            GatewayError::ConfigError(_)
            | GatewayError::Internal(_)
            | GatewayError::Sqlx(_)
            | GatewayError::Migrate(_) => (StatusCode::INTERNAL_SERVER_ERROR, self.to_string()),
        };

        let body = json!({ "error": message });
        let mut response = (status, axum::Json(body)).into_response();
        if circuit_open {
            response
                .headers_mut()
                .insert("X-Lynxx-Circuit", HeaderValue::from_static("open"));
        }
        response
    }
}

impl From<PluginError> for GatewayError {
    fn from(e: PluginError) -> Self {
        GatewayError::PluginError(e)
    }
}
