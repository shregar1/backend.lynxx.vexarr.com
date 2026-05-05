use crate::error::{GatewayError, PluginError};
use crate::plugins::{BoxBody, Plugin};
use async_trait::async_trait;
use axum::http::{Request, Response};
use rand::Rng;
use serde_json::json;
use std::time::Duration;
use tokio::sync::mpsc;

const HDR_METHOD: &str = "x-lynxx-honeycomb-method";
const HDR_PATH: &str = "x-lynxx-honeycomb-path";
const HDR_START: &str = "x-lynxx-honeycomb-start";
const HDR_REQID: &str = "x-lynxx-honeycomb-reqid";
const HDR_IP: &str = "x-lynxx-honeycomb-ip";
const HDR_CONSUMER: &str = "x-consumer-username";

/// Send per-request events to Honeycomb's `/1/batch/{dataset}` endpoint.
///
/// One in-process buffer + background flusher per plugin instance. Events
/// are sampled before they hit the channel so we don't waste capacity on
/// dropped events. A failing endpoint logs at WARN and never blocks the
/// proxy.
pub struct HoneycombPlugin {
    tx: mpsc::Sender<Event>,
    sample_rate: u32,
}

#[derive(Debug, Clone)]
struct Event {
    time: String,
    request_id: String,
    method: String,
    path: String,
    status: u16,
    duration_ms: i64,
    consumer: String,
    ip: String,
}

impl HoneycombPlugin {
    pub fn from_config(cfg: &serde_json::Value) -> Result<Self, GatewayError> {
        let api_key = cfg
            .get("api_key")
            .and_then(|v| v.as_str())
            .ok_or_else(|| GatewayError::BadRequest("honeycomb: `api_key` is required".into()))?
            .to_string();
        let dataset = cfg
            .get("dataset")
            .and_then(|v| v.as_str())
            .unwrap_or("lynxx")
            .to_string();
        // The `endpoint` config value is the base; we batch via /1/batch/{dataset}
        // for higher throughput. If a full URL with /1/events/<dataset> is
        // provided we strip the suffix to compute the base.
        let endpoint_raw = cfg
            .get("endpoint")
            .and_then(|v| v.as_str())
            .unwrap_or("https://api.honeycomb.io")
            .to_string();
        let base = strip_events_suffix(&endpoint_raw);
        let batch_url = format!("{}/1/batch/{}", base.trim_end_matches('/'), dataset);

        let sample_rate = cfg
            .get("sample_rate")
            .and_then(|v| v.as_u64())
            .map(|n| n.max(1) as u32)
            .unwrap_or(1);

        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .map_err(|e| GatewayError::ConfigError(format!("honeycomb: build client: {e}")))?;

        let (tx, rx) = mpsc::channel::<Event>(8192);
        let sr = sample_rate;
        tokio::spawn(async move {
            forwarder_task(client, batch_url, api_key, sr, rx).await;
        });

        Ok(Self { tx, sample_rate })
    }
}

fn strip_events_suffix(s: &str) -> String {
    // Trim a trailing /1/events/<dataset> if the operator pasted the per-event URL.
    let parts: Vec<&str> = s.rsplitn(4, '/').collect();
    if parts.len() == 4 && parts[2] == "events" && parts[3].ends_with("/1") {
        // parts[3] looks like "https://host/1"
        let trimmed = parts[3].trim_end_matches("/1");
        return trimmed.to_string();
    }
    s.to_string()
}

async fn forwarder_task(
    client: reqwest::Client,
    url: String,
    api_key: String,
    sample_rate: u32,
    mut rx: mpsc::Receiver<Event>,
) {
    let mut buf: Vec<Event> = Vec::with_capacity(64);
    let mut ticker = tokio::time::interval(Duration::from_millis(1000));

    loop {
        tokio::select! {
            biased;
            maybe = rx.recv() => {
                match maybe {
                    Some(ev) => {
                        buf.push(ev);
                        if buf.len() >= 100 {
                            flush(&client, &url, &api_key, sample_rate, &mut buf).await;
                        }
                    }
                    None => {
                        if !buf.is_empty() {
                            flush(&client, &url, &api_key, sample_rate, &mut buf).await;
                        }
                        return;
                    }
                }
            }
            _ = ticker.tick() => {
                if !buf.is_empty() {
                    flush(&client, &url, &api_key, sample_rate, &mut buf).await;
                }
            }
        }
    }
}

async fn flush(
    client: &reqwest::Client,
    url: &str,
    api_key: &str,
    sample_rate: u32,
    buf: &mut Vec<Event>,
) {
    // Honeycomb /1/batch/{dataset} accepts a JSON array of `{time, samplerate, data}`.
    let arr: Vec<serde_json::Value> = buf
        .drain(..)
        .map(|ev| {
            json!({
                "time": ev.time,
                "samplerate": sample_rate,
                "data": {
                    "request_id": ev.request_id,
                    "method": ev.method,
                    "path": ev.path,
                    "status": ev.status,
                    "duration_ms": ev.duration_ms,
                    "service": "lynxx",
                    "consumer": ev.consumer,
                    "ip": ev.ip,
                }
            })
        })
        .collect();

    let res = client
        .post(url)
        .header("X-Honeycomb-Team", api_key)
        .header("Content-Type", "application/json")
        .json(&arr)
        .send()
        .await;
    match res {
        Ok(r) if r.status().is_success() => {}
        Ok(r) => tracing::warn!(status = %r.status(), "honeycomb: non-2xx"),
        Err(e) => tracing::warn!(error = %e, "honeycomb: send failed"),
    }
}

#[async_trait]
impl Plugin for HoneycombPlugin {
    fn name(&self) -> &str {
        "honeycomb"
    }

    async fn on_request(&self, req: &mut Request<axum::body::Body>) -> Result<(), PluginError> {
        let method = req.method().as_str().to_string();
        let path = req.uri().path().to_string();
        let now_ms = chrono::Utc::now().timestamp_millis();
        let request_id = req
            .headers()
            .get("x-request-id")
            .and_then(|v| v.to_str().ok())
            .map(String::from)
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        let ip = req
            .headers()
            .get("x-forwarded-for")
            .and_then(|v| v.to_str().ok())
            .map(String::from)
            .unwrap_or_default();
        if let Ok(v) = axum::http::HeaderValue::from_str(&method) {
            req.headers_mut().insert(HDR_METHOD, v);
        }
        if let Ok(v) = axum::http::HeaderValue::from_str(&path) {
            req.headers_mut().insert(HDR_PATH, v);
        }
        if let Ok(v) = axum::http::HeaderValue::from_str(&now_ms.to_string()) {
            req.headers_mut().insert(HDR_START, v);
        }
        if let Ok(v) = axum::http::HeaderValue::from_str(&request_id) {
            req.headers_mut().insert(HDR_REQID, v);
        }
        if let Ok(v) = axum::http::HeaderValue::from_str(&ip) {
            req.headers_mut().insert(HDR_IP, v);
        }
        Ok(())
    }

    async fn on_response(&self, resp: &mut Response<BoxBody>) -> Result<(), PluginError> {
        // Sample 1-in-N. We sample at enqueue time so a low-volume endpoint
        // doesn't pay for events that will be dropped server-side.
        if self.sample_rate > 1 {
            let n = self.sample_rate;
            let pick = rand::thread_rng().gen_range(0..n);
            if pick != 0 {
                resp.headers_mut().remove(HDR_METHOD);
                resp.headers_mut().remove(HDR_PATH);
                resp.headers_mut().remove(HDR_START);
                resp.headers_mut().remove(HDR_REQID);
                resp.headers_mut().remove(HDR_IP);
                return Ok(());
            }
        }
        let method = resp
            .headers()
            .get(HDR_METHOD)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("?")
            .to_string();
        let path = resp
            .headers()
            .get(HDR_PATH)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("?")
            .to_string();
        let start_ms = resp
            .headers()
            .get(HDR_START)
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.parse::<i64>().ok())
            .unwrap_or_else(|| chrono::Utc::now().timestamp_millis());
        let request_id = resp
            .headers()
            .get(HDR_REQID)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        let ip = resp
            .headers()
            .get(HDR_IP)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        let consumer = resp
            .headers()
            .get(HDR_CONSUMER)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        resp.headers_mut().remove(HDR_METHOD);
        resp.headers_mut().remove(HDR_PATH);
        resp.headers_mut().remove(HDR_START);
        resp.headers_mut().remove(HDR_REQID);
        resp.headers_mut().remove(HDR_IP);

        let now = chrono::Utc::now();
        let ev = Event {
            time: now.to_rfc3339(),
            request_id,
            method,
            path,
            status: resp.status().as_u16(),
            duration_ms: (now.timestamp_millis() - start_ms).max(0),
            consumer,
            ip,
        };
        if let Err(mpsc::error::TrySendError::Full(_)) = self.tx.try_send(ev) {
            tracing::debug!("honeycomb: channel full; dropping event");
        }
        Ok(())
    }
}
