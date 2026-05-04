use crate::error::{GatewayError, PluginError};
use crate::plugins::{BoxBody, Plugin};
use async_trait::async_trait;
use axum::http::{Request, Response};

pub struct BotDetectionPlugin {
    deny_user_agents_lower: Vec<String>,
}

impl BotDetectionPlugin {
    pub fn from_config(cfg: &serde_json::Value) -> Result<Self, GatewayError> {
        let list = cfg
            .get("deny_user_agents")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_lowercase()))
                    .collect()
            })
            .unwrap_or_default();
        Ok(Self {
            deny_user_agents_lower: list,
        })
    }
}

#[async_trait]
impl Plugin for BotDetectionPlugin {
    fn name(&self) -> &str {
        "bot-detection"
    }

    async fn on_request(
        &self,
        req: &mut Request<axum::body::Body>,
    ) -> Result<(), PluginError> {
        let ua = req
            .headers()
            .get(axum::http::header::USER_AGENT)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_lowercase();

        if self
            .deny_user_agents_lower
            .iter()
            .any(|needle| !needle.is_empty() && ua.contains(needle))
        {
            return Err(PluginError::Forbidden("user-agent denied".into()));
        }

        Ok(())
    }

    async fn on_response(&self, _resp: &mut Response<BoxBody>) -> Result<(), PluginError> {
        Ok(())
    }
}
