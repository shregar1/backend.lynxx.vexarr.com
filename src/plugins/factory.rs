use crate::db::Repository;
use crate::error::GatewayError;
use crate::plugins::acl::AclPlugin;
use crate::plugins::auth::AuthPlugin;
use crate::plugins::aws_lambda::AwsLambdaPlugin;
use crate::plugins::basic_auth::BasicAuthPlugin;
use crate::plugins::bot_detection::BotDetectionPlugin;
use crate::plugins::canary::CanaryPlugin;
use crate::plugins::correlation_id::CorrelationIdPlugin;
use crate::plugins::cors::CorsPlugin;
use crate::plugins::datadog::DatadogPlugin;
use crate::plugins::file_log::FileLogPlugin;
use crate::plugins::graphql_guard::GraphqlGuardPlugin;
use crate::plugins::grpc_web::GrpcWebPlugin;
use crate::plugins::hmac_auth::HmacAuthPlugin;
use crate::plugins::honeycomb::HoneycombPlugin;
use crate::plugins::ip_restriction::IpRestrictionPlugin;
use crate::plugins::jwt::JwtPlugin;
use crate::plugins::key_auth::KeyAuthPlugin;
use crate::plugins::llm_gateway::LlmGatewayPlugin;
use crate::plugins::logging::LoggingPlugin;
use crate::plugins::mocking::MockingPlugin;
use crate::plugins::oauth2_introspect::Oauth2IntrospectPlugin;
use crate::plugins::openapi_drift::OpenApiDriftPlugin;
use crate::plugins::openapi_mock::OpenApiMockPlugin;
use crate::plugins::opentelemetry::OpenTelemetryPlugin;
use crate::plugins::prometheus_per_consumer::PrometheusPerConsumerPlugin;
use crate::plugins::proxy_cache::ProxyCachePlugin;
use crate::plugins::rate_limit::RateLimitPlugin;
use crate::plugins::redis_rate_limit::RedisRateLimitPlugin;
use crate::plugins::request_cleaner::RequestCleanerPlugin;
use crate::plugins::request_id::RequestIdPlugin;
use crate::plugins::request_recorder::RequestRecorderPlugin;
use crate::plugins::request_size_limit::RequestSizeLimitPlugin;
use crate::plugins::request_termination::RequestTerminationPlugin;
use crate::plugins::request_transformer::RequestTransformerPlugin;
use crate::plugins::request_validator::RequestValidatorPlugin;
use crate::plugins::response_cleaner::ResponseCleanerPlugin;
use crate::plugins::response_ratelimiting::ResponseRateLimitingPlugin;
use crate::plugins::response_transformer::ResponseTransformerPlugin;
use crate::plugins::session::SessionPlugin;
use crate::plugins::shadow::ShadowPlugin;
use crate::plugins::splunk::SplunkPlugin;
use crate::plugins::statsd::StatsdPlugin;
use crate::plugins::syslog::SyslogPlugin;
use crate::plugins::wasm::WasmPlugin;
use crate::plugins::websocket_size_limit::WebsocketSizeLimitPlugin;
use crate::plugins::Plugin;
use std::sync::Arc;

/// Construct a plugin from its persisted name + config blob. `repo` is
/// optional and only consulted by plugins that need DB access (e.g. consumer
/// lookup for `basic-auth` and `key-auth`). Plugins that don't need it just
/// ignore the parameter.
pub fn build(
    name: &str,
    config: &serde_json::Value,
    repo: Option<Arc<dyn Repository>>,
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
        "oauth2-introspect" => Arc::new(Oauth2IntrospectPlugin::from_config(config, repo)?),
        "hmac-auth" => Arc::new(HmacAuthPlugin::from_config(config, repo)?),
        "acl" => Arc::new(AclPlugin::from_config(config, repo)?),
        "request-validator" => Arc::new(RequestValidatorPlugin::from_config(config, repo)?),
        "session" => Arc::new(SessionPlugin::from_config(config, repo)?),
        "statsd" => Arc::new(StatsdPlugin::from_config(config)?),
        "datadog" => Arc::new(DatadogPlugin::from_config(config)?),
        "redis-rate-limit" => Arc::new(RedisRateLimitPlugin::from_config(config)?),
        "opentelemetry" => Arc::new(OpenTelemetryPlugin::from_config(config)?),
        "syslog" => Arc::new(SyslogPlugin::from_config(config)?),
        "file-log" => Arc::new(FileLogPlugin::from_config(config)?),
        "aws-lambda" => Arc::new(AwsLambdaPlugin::from_config(config)?),
        "prometheus-per-consumer" => Arc::new(PrometheusPerConsumerPlugin::from_config(config)?),
        "request-id" => Arc::new(RequestIdPlugin::from_config(config)?),
        "request-recorder" => Arc::new(RequestRecorderPlugin::from_config(config, repo)?),
        "correlation-id" => Arc::new(CorrelationIdPlugin::from_config(config)?),
        "splunk" => Arc::new(SplunkPlugin::from_config(config)?),
        "honeycomb" => Arc::new(HoneycombPlugin::from_config(config)?),
        "grpc-web" => Arc::new(GrpcWebPlugin::from_config(config)?),
        "mocking" => Arc::new(MockingPlugin::from_config(config)?),
        "response-ratelimiting" => Arc::new(ResponseRateLimitingPlugin::from_config(config)?),
        "request-cleaner" => Arc::new(RequestCleanerPlugin::from_config(config)?),
        "response-cleaner" => Arc::new(ResponseCleanerPlugin::from_config(config)?),
        "canary" => Arc::new(CanaryPlugin::from_config(config)?),
        "shadow" => Arc::new(ShadowPlugin::from_config(config)?),
        "websocket-size-limit" => Arc::new(WebsocketSizeLimitPlugin::from_config(config)?),
        "llm-gateway" => Arc::new(LlmGatewayPlugin::from_config(config)?),
        "graphql-guard" => Arc::new(GraphqlGuardPlugin::from_config(config)?),
        "wasm" => Arc::new(WasmPlugin::from_config(config)?),
        "openapi-drift" => Arc::new(OpenApiDriftPlugin::from_config(config)?),
        "openapi-mock" => Arc::new(OpenApiMockPlugin::from_config(config)?),
        other => {
            return Err(GatewayError::BadRequest(format!("unknown plugin: {other}")));
        }
    };
    Ok(plugin)
}
