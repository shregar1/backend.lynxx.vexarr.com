use crate::error::GatewayError;
use crate::plugins::auth::AuthPlugin;
use crate::plugins::bot_detection::BotDetectionPlugin;
use crate::plugins::cors::CorsPlugin;
use crate::plugins::ip_restriction::IpRestrictionPlugin;
use crate::plugins::logging::LoggingPlugin;
use crate::plugins::rate_limit::RateLimitPlugin;
use crate::plugins::request_size_limit::RequestSizeLimitPlugin;
use crate::plugins::request_transformer::RequestTransformerPlugin;
use crate::plugins::response_transformer::ResponseTransformerPlugin;
use crate::plugins::Plugin;
use std::sync::Arc;

/// Construct a plugin from its persisted name + config blob.
pub fn build(name: &str, config: &serde_json::Value) -> Result<Arc<dyn Plugin>, GatewayError> {
    let plugin: Arc<dyn Plugin> = match name {
        "rate-limit" => Arc::new(RateLimitPlugin::from_config(config)?),
        "cors" => Arc::new(CorsPlugin::from_config(config)?),
        "logging" => Arc::new(LoggingPlugin::from_config(config)?),
        "auth" => Arc::new(AuthPlugin::from_config(config)?),
        "ip-restriction" => Arc::new(IpRestrictionPlugin::from_config(config)?),
        "request-size-limit" => Arc::new(RequestSizeLimitPlugin::from_config(config)?),
        "request-transformer" => Arc::new(RequestTransformerPlugin::from_config(config)?),
        "response-transformer" => Arc::new(ResponseTransformerPlugin::from_config(config)?),
        "bot-detection" => Arc::new(BotDetectionPlugin::from_config(config)?),
        other => {
            return Err(GatewayError::BadRequest(format!(
                "unknown plugin: {other}"
            )));
        }
    };
    Ok(plugin)
}
