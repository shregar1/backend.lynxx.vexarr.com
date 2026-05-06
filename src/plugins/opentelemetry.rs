use crate::error::{GatewayError, PluginError};
use crate::plugins::{BoxBody, Plugin};
use async_trait::async_trait;
use axum::http::header::HeaderValue;
use axum::http::{Request, Response};
use serde_json::json;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const HDR_TRACE_ID: &str = "x-lynxx-otel-trace";
const HDR_SPAN_ID: &str = "x-lynxx-otel-span";
const HDR_START_NS: &str = "x-lynxx-otel-start";

/// OpenTelemetry exporter (OTLP/HTTP, JSON-encoded). Builds spans manually
/// to avoid pulling the heavyweight `opentelemetry-otlp` dependency tree;
/// the on-the-wire format is the same.
pub struct OpenTelemetryPlugin {
    endpoint: String,
    service_name: String,
    sample_rate: f64,
    http: reqwest::Client,
}

impl OpenTelemetryPlugin {
    pub fn from_config(cfg: &serde_json::Value) -> Result<Self, GatewayError> {
        let endpoint = cfg
            .get("endpoint")
            .and_then(|v| v.as_str())
            .unwrap_or("http://otel-collector:4318/v1/traces")
            .to_string();
        let service_name = cfg
            .get("service_name")
            .and_then(|v| v.as_str())
            .unwrap_or("lynxx-gateway")
            .to_string();
        let sample_rate = cfg
            .get("sample_rate")
            .and_then(|v| v.as_f64())
            .unwrap_or(1.0)
            .clamp(0.0, 1.0);
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
            .map_err(|e| GatewayError::ConfigError(format!("otel build client: {e}")))?;
        Ok(Self {
            endpoint,
            service_name,
            sample_rate,
            http,
        })
    }

    fn rand_hex(bytes: usize) -> String {
        // Tiny xorshift seeded from system nanos. Good enough for trace ids;
        // we don't need cryptographic randomness here.
        let seed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0xdead_beef);
        let mut s = seed ^ (std::ptr::addr_of!(seed) as u64);
        let mut out = String::with_capacity(bytes * 2);
        for _ in 0..bytes {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            let b = (s & 0xff) as u8;
            out.push_str(&format!("{:02x}", b));
        }
        out
    }

    fn now_unix_nano() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0)
    }

    fn parse_traceparent(s: &str) -> Option<(String, String)> {
        // Format: 00-<32hex>-<16hex>-<2hex>
        let parts: Vec<&str> = s.split('-').collect();
        if parts.len() != 4 {
            return None;
        }
        if parts[1].len() != 32 || parts[2].len() != 16 {
            return None;
        }
        Some((parts[1].to_string(), parts[2].to_string()))
    }
}

#[async_trait]
impl Plugin for OpenTelemetryPlugin {
    fn name(&self) -> &str {
        "opentelemetry"
    }

    async fn on_request(&self, req: &mut Request<axum::body::Body>) -> Result<(), PluginError> {
        // Sampling decision per request; if dropped, leave any existing
        // traceparent untouched.
        let sample = {
            let r = (Self::now_unix_nano() % 1_000_000) as f64 / 1_000_000.0;
            r < self.sample_rate
        };
        if !sample {
            return Ok(());
        }

        let (trace_id, span_id) = match req
            .headers()
            .get("traceparent")
            .and_then(|v| v.to_str().ok())
            .and_then(Self::parse_traceparent)
        {
            Some((tid, _parent_span)) => (tid, Self::rand_hex(8)),
            None => (Self::rand_hex(16), Self::rand_hex(8)),
        };

        let traceparent = format!("00-{trace_id}-{span_id}-01");
        if let Ok(v) = HeaderValue::from_str(&traceparent) {
            req.headers_mut().insert("traceparent", v);
        }
        if let Ok(v) = HeaderValue::from_str(&trace_id) {
            req.headers_mut().insert(HDR_TRACE_ID, v);
        }
        if let Ok(v) = HeaderValue::from_str(&span_id) {
            req.headers_mut().insert(HDR_SPAN_ID, v);
        }
        let start = Self::now_unix_nano();
        if let Ok(v) = HeaderValue::from_str(&start.to_string()) {
            req.headers_mut().insert(HDR_START_NS, v);
        }
        Ok(())
    }

    async fn on_response(&self, resp: &mut Response<BoxBody>) -> Result<(), PluginError> {
        let trace_id = match resp
            .headers()
            .get(HDR_TRACE_ID)
            .and_then(|v| v.to_str().ok())
        {
            Some(t) => t.to_string(),
            None => return Ok(()),
        };
        let span_id = resp
            .headers()
            .get(HDR_SPAN_ID)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        let start = resp
            .headers()
            .get(HDR_START_NS)
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or_else(Self::now_unix_nano);
        resp.headers_mut().remove(HDR_TRACE_ID);
        resp.headers_mut().remove(HDR_SPAN_ID);
        resp.headers_mut().remove(HDR_START_NS);

        let end = Self::now_unix_nano();
        let status = resp.status().as_u16();

        let payload = json!({
            "resourceSpans": [{
                "resource": {
                    "attributes": [
                        { "key": "service.name", "value": { "stringValue": self.service_name } }
                    ]
                },
                "scopeSpans": [{
                    "scope": { "name": "lynxx-gateway" },
                    "spans": [{
                        "traceId": trace_id,
                        "spanId": span_id,
                        "name": "http.request",
                        "kind": 2, // SPAN_KIND_SERVER
                        "startTimeUnixNano": start.to_string(),
                        "endTimeUnixNano": end.to_string(),
                        "attributes": [
                            { "key": "http.status_code", "value": { "intValue": status.to_string() } },
                            { "key": "service", "value": { "stringValue": self.service_name } }
                        ],
                        "status": {
                            "code": if status >= 500 { 2 } else { 1 }
                        }
                    }]
                }]
            }]
        });

        let endpoint = self.endpoint.clone();
        let http: Arc<reqwest::Client> = Arc::new(self.http.clone());
        tokio::spawn(async move {
            let res = http
                .post(&endpoint)
                .header("content-type", "application/json")
                .body(payload.to_string())
                .send()
                .await;
            if let Err(e) = res {
                tracing::debug!(error = %e, "otel export failed");
            }
        });
        Ok(())
    }
}
