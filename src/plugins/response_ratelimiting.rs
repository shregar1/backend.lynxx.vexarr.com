use crate::error::{GatewayError, PluginError};
use crate::plugins::{BoxBody, Plugin};
use async_trait::async_trait;
use axum::http::{Request, Response};
use dashmap::DashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

const HDR_CONSUMER: &str = "x-consumer-username";
const HDR_SERVICE: &str = "x-lynxx-service-name";
const HDR_KEY: &str = "x-lynxx-respratelimit-key";

/// Throttle responses per bucket key.
///
/// **v1 implementation note:** byte-rate streaming is the long-term goal but
/// requires plumbing into the proxy body forwarder, which is out of scope for
/// this revision. Instead we apply a simple count-based limiter: at most `N`
/// responses per consumer per second, where `N` is derived from
/// `limit_bytes_per_second / 1024`. This is documented to operators as a
/// pragmatic stopgap; bytes-based throttling is on the v2 roadmap.
pub struct ResponseRateLimitingPlugin {
    limit_per_second: u64,
    key_strategy: KeyStrategy,
    state: Arc<DashMap<String, Bucket>>,
}

#[derive(Clone, Copy)]
enum KeyStrategy {
    Consumer,
    Service,
    Global,
}

#[derive(Default)]
struct Bucket {
    /// Start of the current 1-second window.
    window_start: Option<Instant>,
    /// Responses observed within the current window.
    count: u64,
}

impl ResponseRateLimitingPlugin {
    pub fn from_config(cfg: &serde_json::Value) -> Result<Self, GatewayError> {
        let bytes = cfg
            .get("limit_bytes_per_second")
            .and_then(|v| v.as_u64())
            .unwrap_or(1_048_576);
        // Convert to ~responses/sec assuming 1KiB average response. Operators
        // can tune via the byte limit; the conversion is intentionally rough.
        let limit_per_second = (bytes / 1024).max(1);
        let key_strategy = match cfg
            .get("key_strategy")
            .and_then(|v| v.as_str())
            .unwrap_or("consumer")
        {
            "service" => KeyStrategy::Service,
            "global" => KeyStrategy::Global,
            _ => KeyStrategy::Consumer,
        };
        Ok(Self {
            limit_per_second,
            key_strategy,
            state: Arc::new(DashMap::new()),
        })
    }

    fn bucket_key(&self, consumer: &str, service: &str) -> String {
        match self.key_strategy {
            KeyStrategy::Consumer => {
                if consumer.is_empty() {
                    "anonymous".to_string()
                } else {
                    consumer.to_string()
                }
            }
            KeyStrategy::Service => {
                if service.is_empty() {
                    "default".to_string()
                } else {
                    service.to_string()
                }
            }
            KeyStrategy::Global => "global".to_string(),
        }
    }
}

#[async_trait]
impl Plugin for ResponseRateLimitingPlugin {
    fn name(&self) -> &str {
        "response-ratelimiting"
    }

    async fn on_request(&self, req: &mut Request<axum::body::Body>) -> Result<(), PluginError> {
        // Stash the chosen key on the request so on_response can use it
        // without re-deriving (and without depending on whether downstream
        // plugins have already removed the consumer header).
        let consumer = req
            .headers()
            .get(HDR_CONSUMER)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        let service = req
            .headers()
            .get(HDR_SERVICE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        let key = self.bucket_key(&consumer, &service);
        if let Ok(v) = axum::http::HeaderValue::from_str(&key) {
            req.headers_mut().insert(HDR_KEY, v);
        }

        // Pre-flight: reject the request up front if this bucket has already
        // exhausted its budget for the current second. This avoids spending
        // upstream effort on a response we'd just have to throttle anyway.
        let now = Instant::now();
        let mut entry = self.state.entry(key).or_default();
        let reset = match entry.window_start {
            Some(t) if now.duration_since(t) < Duration::from_secs(1) => false,
            _ => true,
        };
        if reset {
            entry.window_start = Some(now);
            entry.count = 0;
        }
        if entry.count >= self.limit_per_second {
            return Err(PluginError::RateLimited);
        }
        entry.count += 1;
        Ok(())
    }

    async fn on_response(&self, resp: &mut Response<BoxBody>) -> Result<(), PluginError> {
        // Surface the limit to the client so they can self-throttle.
        resp.headers_mut().remove(HDR_KEY);
        if let Ok(v) = axum::http::HeaderValue::from_str(&self.limit_per_second.to_string()) {
            resp.headers_mut()
                .insert("x-ratelimit-response-limit", v);
        }
        Ok(())
    }
}
