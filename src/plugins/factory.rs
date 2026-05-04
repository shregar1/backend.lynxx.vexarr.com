use crate::db::Repository;
use crate::error::GatewayError;
use crate::plugins::auth::AuthPlugin;
use crate::plugins::basic_auth::BasicAuthPlugin;
use crate::plugins::bot_detection::BotDetectionPlugin;
use crate::plugins::cors::CorsPlugin;
use crate::plugins::ip_restriction::IpRestrictionPlugin;
use crate::plugins::jwt::JwtPlugin;
use crate::plugins::key_auth::KeyAuthPlugin;
use crate::plugins::logging::LoggingPlugin;
use crate::plugins::proxy_cache::ProxyCachePlugin;
use crate::plugins::rate_limit::RateLimitPlugin;
use crate::plugins::request_size_limit::RequestSizeLimitPlugin;
use crate::plugins::request_termination::RequestTerminationPlugin;
use crate::plugins::request_transformer::RequestTransformerPlugin;
use crate::plugins::response_transformer::ResponseTransformerPlugin;
use crate::plugins::Plugin;
use std::sync::Arc;

/// Construct a plugin from its persisted name + config blob. `repo` is
/// optional and only consulted by plugins that need DB access (e.g. consumer
/// lookup for `basic-auth` and `key-auth`). Plugins that don't need it just
/// ignore the parameter.
pub fn build(
    name: &str,
    config: &serde_json::Value,
    repo: Option<Arc<Repository>>,
) -> Result<Arc<dyn Plugin>, GatewayError> {
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
        "jwt" => Arc::new(JwtPlugin::from_config(config)?),
        "basic-auth" => Arc::new(BasicAuthPlugin::from_config(config, repo)?),
        "key-auth" => Arc::new(KeyAuthPlugin::from_config(config, repo)?),
        "proxy-cache" => Arc::new(ProxyCachePlugin::from_config(config)?),
        "request-termination" => Arc::new(RequestTerminationPlugin::from_config(config)?),
        other => {
            return Err(GatewayError::BadRequest(format!(
                "unknown plugin: {other}"
            )));
        }
    };
    Ok(plugin)
}
