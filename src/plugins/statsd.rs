use crate::error::{GatewayError, PluginError};
use crate::plugins::{BoxBody, Plugin};
use async_trait::async_trait;
use axum::http::{Request, Response};
use std::sync::Arc;
use std::time::Instant;
use tokio::net::UdpSocket;

const HEADER_START_INSTANT: &str = "x-lynxx-statsd-start";

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Metric {
    RequestCount,
    RequestDuration,
    StatusCount,
    RequestSize,
    ResponseSize,
}

impl Metric {
    fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "request_count" => Self::RequestCount,
            "request_duration" => Self::RequestDuration,
            "status_count" => Self::StatusCount,
            "request_size" => Self::RequestSize,
            "response_size" => Self::ResponseSize,
            _ => return None,
        })
    }
}

/// StatsD metric reporter. Emits counters and timings via UDP after each
/// response. Failures are logged at debug level and never affect request flow.
pub struct StatsdPlugin {
    socket: Arc<UdpSocket>,
    target: String,
    prefix: String,
    tags: Vec<String>,
    metrics: Vec<Metric>,
    /// When true, encode tags in DogStatsD format (`|#tag:val,...`). Otherwise
    /// tags are appended as a comma-separated suffix (vanilla StatsD has no
    /// official tag format; many backends accept the dogstatsd shape anyway).
    dogstatsd: bool,
    service_tag_key: String,
}

impl StatsdPlugin {
    pub fn from_config(cfg: &serde_json::Value) -> Result<Self, GatewayError> {
        Self::build(cfg, false, "service")
    }

    pub fn build(
        cfg: &serde_json::Value,
        dogstatsd: bool,
        default_service_tag_key: &str,
    ) -> Result<Self, GatewayError> {
        let host = cfg
            .get("host")
            .and_then(|v| v.as_str())
            .unwrap_or("127.0.0.1")
            .to_string();
        let port = cfg.get("port").and_then(|v| v.as_u64()).unwrap_or(8125) as u16;
        let prefix = cfg
            .get("prefix")
            .and_then(|v| v.as_str())
            .unwrap_or("lynxx")
            .to_string();
        let tags = cfg
            .get("tags")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let metrics = cfg
            .get("metrics")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().and_then(Metric::parse))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_else(|| {
                vec![
                    Metric::RequestCount,
                    Metric::RequestDuration,
                    Metric::StatusCount,
                ]
            });
        let service_tag_key = cfg
            .get("service_tag_key")
            .and_then(|v| v.as_str())
            .unwrap_or(default_service_tag_key)
            .to_string();

        // Bind once at construction. The OS picks an ephemeral source port;
        // we connect() so subsequent send() calls don't need an addr.
        let socket = std::net::UdpSocket::bind("0.0.0.0:0")
            .map_err(|e| GatewayError::ConfigError(format!("statsd bind: {e}")))?;
        socket
            .set_nonblocking(true)
            .map_err(|e| GatewayError::ConfigError(format!("statsd nonblocking: {e}")))?;
        let target = format!("{host}:{port}");
        socket
            .connect(&target)
            .map_err(|e| GatewayError::ConfigError(format!("statsd connect {target}: {e}")))?;
        let socket = UdpSocket::from_std(socket)
            .map_err(|e| GatewayError::ConfigError(format!("statsd tokio adopt: {e}")))?;

        Ok(Self {
            socket: Arc::new(socket),
            target,
            prefix,
            tags,
            metrics,
            dogstatsd,
            service_tag_key,
        })
    }

    fn format_tags(&self, extra: &[(&str, String)]) -> String {
        let mut all: Vec<String> = self.tags.clone();
        for (k, v) in extra {
            all.push(format!("{k}:{v}"));
        }
        if all.is_empty() {
            return String::new();
        }
        if self.dogstatsd {
            format!("|#{}", all.join(","))
        } else {
            // Many statsd servers (telegraf, etc.) also accept dogstatsd
            // tag style, so we emit the same suffix unconditionally.
            format!("|#{}", all.join(","))
        }
    }

    async fn send(&self, line: String) {
        if let Err(e) = self.socket.send(line.as_bytes()).await {
            tracing::debug!(target = %self.target, error = %e, line = %line, "statsd send failed");
        }
    }

    pub(crate) fn enabled(&self, m: Metric) -> bool {
        self.metrics.contains(&m)
    }

    pub(crate) fn prefix(&self) -> &str {
        &self.prefix
    }

    pub(crate) fn service_tag_key(&self) -> &str {
        &self.service_tag_key
    }
}

#[async_trait]
impl Plugin for StatsdPlugin {
    fn name(&self) -> &str {
        "statsd"
    }

    async fn on_request(&self, req: &mut Request<axum::body::Body>) -> Result<(), PluginError> {
        // Stash start time as nanos so on_response can compute duration.
        let now_ns = Instant::now().elapsed().as_nanos();
        // Use a monotonic absolute reference: we measure delta between two
        // `Instant::now()` reads, so storing wall-clock-ish nanos won't work
        // across the request boundary. Instead store the start-of-process
        // anchor by encoding the duration since an epoch held in a OnceLock.
        let _ = now_ns;
        let anchor = anchor();
        let elapsed = anchor.elapsed().as_nanos() as u64;
        if let Ok(v) = axum::http::HeaderValue::from_str(&elapsed.to_string()) {
            req.headers_mut().insert(HEADER_START_INSTANT, v);
        }
        Ok(())
    }

    async fn on_response(&self, resp: &mut Response<BoxBody>) -> Result<(), PluginError> {
        let start_ns = resp
            .headers()
            .get(HEADER_START_INSTANT)
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.parse::<u64>().ok());
        resp.headers_mut().remove(HEADER_START_INSTANT);

        let status = resp.status().as_u16();
        let duration_ms = start_ns.map(|s| {
            let now = anchor().elapsed().as_nanos() as u64;
            now.saturating_sub(s) as f64 / 1_000_000.0
        });

        let extra = vec![(self.service_tag_key.as_str(), format!("{status}"))];
        let _ = extra; // placeholder; we tag per-metric below.

        let prefix = self.prefix.clone();
        let socket = self.socket.clone();
        let tags = self.tags.clone();
        let dogstatsd = self.dogstatsd;
        let metrics = self.metrics.clone();
        let service_tag_key = self.service_tag_key.clone();

        // Fire-and-forget: don't block the response on UDP I/O.
        tokio::spawn(async move {
            let format_tags = |extra: Vec<(String, String)>| -> String {
                let mut all = tags.clone();
                for (k, v) in extra {
                    all.push(format!("{k}:{v}"));
                }
                if all.is_empty() {
                    String::new()
                } else if dogstatsd {
                    format!("|#{}", all.join(","))
                } else {
                    format!("|#{}", all.join(","))
                }
            };

            let send_line = |line: String| {
                let s = socket.clone();
                async move {
                    if let Err(e) = s.send(line.as_bytes()).await {
                        tracing::debug!(error = %e, "statsd send failed");
                    }
                }
            };

            if metrics.contains(&Metric::RequestCount) {
                let suffix = format_tags(vec![(service_tag_key.clone(), status.to_string())]);
                send_line(format!("{prefix}.requests:1|c{suffix}")).await;
            }
            if metrics.contains(&Metric::StatusCount) {
                let class = format!("{}xx", status / 100);
                let suffix =
                    format_tags(vec![("status_class".into(), class), ("status".into(), status.to_string())]);
                send_line(format!("{prefix}.status:1|c{suffix}")).await;
            }
            if metrics.contains(&Metric::RequestDuration) {
                if let Some(ms) = duration_ms {
                    let suffix = format_tags(vec![(service_tag_key.clone(), status.to_string())]);
                    send_line(format!("{prefix}.latency:{ms:.3}|ms{suffix}")).await;
                }
            }
        });

        Ok(())
    }
}

fn anchor() -> Instant {
    use std::sync::OnceLock;
    static ANCHOR: OnceLock<Instant> = OnceLock::new();
    *ANCHOR.get_or_init(Instant::now)
}
