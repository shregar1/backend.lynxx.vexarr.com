use crate::error::{GatewayError, PluginError};
use crate::plugins::{BoxBody, Plugin};
use async_trait::async_trait;
use axum::http::{Request, Response};
use std::time::Instant;

const HDR_START: &str = "x-lynxx-pc-start";
const HDR_CONSUMER: &str = "x-consumer-username";
const HDR_HOST: &str = "x-lynxx-pc-host";

/// Adds per-consumer dimensions on top of the gateway's existing Prometheus
/// metrics. Reads the consumer from `X-Consumer-Username` (set by auth
/// plugins). Falls back to `anonymous` when no auth plugin ran.
pub struct PrometheusPerConsumerPlugin;

impl PrometheusPerConsumerPlugin {
    pub fn from_config(_cfg: &serde_json::Value) -> Result<Self, GatewayError> {
        Ok(Self)
    }
}

#[async_trait]
impl Plugin for PrometheusPerConsumerPlugin {
    fn name(&self) -> &str {
        "prometheus-per-consumer"
    }

    async fn on_request(&self, req: &mut Request<axum::body::Body>) -> Result<(), PluginError> {
        // Wall-clock as ms-since-epoch is fine here — we only need a delta.
        let now_ms = chrono::Utc::now().timestamp_millis();
        if let Ok(v) = axum::http::HeaderValue::from_str(&now_ms.to_string()) {
            req.headers_mut().insert(HDR_START, v);
        }
        if let Some(host) = req.headers().get("host").cloned() {
            req.headers_mut().insert(HDR_HOST, host);
        }
        let _ = Instant::now();
        Ok(())
    }

    async fn on_response(&self, resp: &mut Response<BoxBody>) -> Result<(), PluginError> {
        let consumer = resp
            .headers()
            .get(HDR_CONSUMER)
            .and_then(|v| v.to_str().ok())
            .map(String::from)
            .unwrap_or_else(|| "anonymous".into());
        let service = resp
            .headers()
            .get(HDR_HOST)
            .and_then(|v| v.to_str().ok())
            .map(String::from)
            .unwrap_or_else(|| "unknown".into());
        let start_ms = resp
            .headers()
            .get(HDR_START)
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.parse::<i64>().ok());
        resp.headers_mut().remove(HDR_START);
        resp.headers_mut().remove(HDR_HOST);

        let status = resp.status().as_u16();
        let class = format!("{}xx", status / 100);

        metrics::counter!(
            "lynxx_consumer_requests_total",
            "consumer" => consumer.clone(),
            "service" => service.clone(),
            "status" => class,
        )
        .increment(1);

        if let Some(start) = start_ms {
            let now = chrono::Utc::now().timestamp_millis();
            let dur_secs = ((now - start).max(0) as f64) / 1000.0;
            metrics::histogram!(
                "lynxx_consumer_request_duration_seconds",
                "consumer" => consumer,
                "service" => service,
            )
            .record(dur_secs);
        }
        Ok(())
    }
}
