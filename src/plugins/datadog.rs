use crate::error::{GatewayError, PluginError};
use crate::plugins::statsd::StatsdPlugin;
use crate::plugins::{BoxBody, Plugin};
use async_trait::async_trait;
use axum::http::{Request, Response};

/// Datadog/DogStatsD reporter. Thin wrapper over the StatsdPlugin that
/// forces dogstatsd-style tag encoding (`|#k:v,...`).
pub struct DatadogPlugin {
    inner: StatsdPlugin,
}

impl DatadogPlugin {
    pub fn from_config(cfg: &serde_json::Value) -> Result<Self, GatewayError> {
        let inner = StatsdPlugin::build(cfg, true, "service")?;
        Ok(Self { inner })
    }
}

#[async_trait]
impl Plugin for DatadogPlugin {
    fn name(&self) -> &str {
        "datadog"
    }

    async fn on_request(&self, req: &mut Request<axum::body::Body>) -> Result<(), PluginError> {
        self.inner.on_request(req).await
    }

    async fn on_response(&self, resp: &mut Response<BoxBody>) -> Result<(), PluginError> {
        self.inner.on_response(resp).await
    }
}
