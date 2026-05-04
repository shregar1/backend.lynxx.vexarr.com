use crate::error::{GatewayError, PluginError};
use crate::plugins::{BoxBody, Plugin};
use async_trait::async_trait;
use axum::http::{Request, Response};

pub struct RequestSizeLimitPlugin {
    max_bytes: u64,
}

impl RequestSizeLimitPlugin {
    pub fn from_config(cfg: &serde_json::Value) -> Result<Self, GatewayError> {
        let max_bytes = cfg
            .get("max_bytes")
            .and_then(|v| v.as_u64())
            .unwrap_or(1_048_576);
        Ok(Self { max_bytes })
    }
}

#[async_trait]
impl Plugin for RequestSizeLimitPlugin {
    fn name(&self) -> &str {
        "request-size-limit"
    }

    async fn on_request(
        &self,
        req: &mut Request<axum::body::Body>,
    ) -> Result<(), PluginError> {
        if let Some(value) = req.headers().get(axum::http::header::CONTENT_LENGTH) {
            if let Some(len) = value.to_str().ok().and_then(|s| s.parse::<u64>().ok()) {
                if len > self.max_bytes {
                    return Err(PluginError::PayloadTooLarge(format!(
                        "{len} > {}",
                        self.max_bytes
                    )));
                }
            }
        }
        Ok(())
    }

    async fn on_response(&self, _resp: &mut Response<BoxBody>) -> Result<(), PluginError> {
        Ok(())
    }
}
