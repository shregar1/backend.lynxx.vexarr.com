use crate::error::{GatewayError, PluginError};
use crate::plugins::{BoxBody, Plugin};
use async_trait::async_trait;
use axum::http::{Request, Response};
use serde_json::json;
use std::time::Duration;
use tokio::sync::mpsc;

const HDR_METHOD: &str = "x-lynxx-splunk-method";
const HDR_PATH: &str = "x-lynxx-splunk-path";
const HDR_START: &str = "x-lynxx-splunk-start";
const HDR_REQID: &str = "x-lynxx-splunk-reqid";
const HDR_CONSUMER: &str = "x-consumer-username";

/// Forward request access logs to a Splunk HTTP Event Collector (HEC). The
/// hot path only `try_send`s a `SplunkEvent` onto a bounded mpsc; a single
/// background task batches and POSTs them. A failing Splunk endpoint logs
/// at WARN and is never allowed to back-pressure the proxy.
pub struct SplunkPlugin {
    tx: mpsc::Sender<SplunkEvent>,
    source: String,
    sourcetype: String,
    index: String,
}

#[derive(Debug, Clone)]
struct SplunkEvent {
    time: f64,
    request_id: String,
    method: String,
    path: String,
    status: u16,
    duration_ms: i64,
    consumer: String,
}

impl SplunkPlugin {
    pub fn from_config(cfg: &serde_json::Value) -> Result<Self, GatewayError> {
        let endpoint = cfg
            .get("endpoint")
            .and_then(|v| v.as_str())
            .ok_or_else(|| GatewayError::BadRequest("splunk: `endpoint` is required".into()))?
            .to_string();
        let token = cfg
            .get("token")
            .and_then(|v| v.as_str())
            .ok_or_else(|| GatewayError::BadRequest("splunk: `token` is required".into()))?
            .to_string();
        let source = cfg
            .get("source")
            .and_then(|v| v.as_str())
            .unwrap_or("lynxx")
            .to_string();
        let sourcetype = cfg
            .get("sourcetype")
            .and_then(|v| v.as_str())
            .unwrap_or("lynxx:access")
            .to_string();
        let index = cfg
            .get("index")
            .and_then(|v| v.as_str())
            .unwrap_or("main")
            .to_string();
        let verify_tls = cfg
            .get("verify_tls")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        let batch_size = cfg
            .get("batch_size")
            .and_then(|v| v.as_u64())
            .unwrap_or(50) as usize;
        let flush_interval_ms = cfg
            .get("flush_interval_ms")
            .and_then(|v| v.as_u64())
            .unwrap_or(1000);

        let client = reqwest::Client::builder()
            .danger_accept_invalid_certs(!verify_tls)
            .timeout(Duration::from_secs(10))
            .build()
            .map_err(|e| GatewayError::ConfigError(format!("splunk: build http client: {e}")))?;

        let (tx, rx) = mpsc::channel::<SplunkEvent>(8192);
        let source_for_task = source.clone();
        let sourcetype_for_task = sourcetype.clone();
        let index_for_task = index.clone();
        tokio::spawn(async move {
            forwarder_task(
                client,
                endpoint,
                token,
                source_for_task,
                sourcetype_for_task,
                index_for_task,
                rx,
                batch_size,
                Duration::from_millis(flush_interval_ms),
            )
            .await;
        });

        Ok(Self {
            tx,
            source,
            sourcetype,
            index,
        })
    }
}

async fn forwarder_task(
    client: reqwest::Client,
    endpoint: String,
    token: String,
    source: String,
    sourcetype: String,
    index: String,
    mut rx: mpsc::Receiver<SplunkEvent>,
    batch_size: usize,
    flush_interval: Duration,
) {
    let host = hostname();
    let auth = format!("Splunk {token}");
    let mut buf: Vec<SplunkEvent> = Vec::with_capacity(batch_size);
    let mut ticker = tokio::time::interval(flush_interval);

    loop {
        tokio::select! {
            biased;
            maybe = rx.recv() => {
                match maybe {
                    Some(ev) => {
                        buf.push(ev);
                        if buf.len() >= batch_size {
                            flush(&client, &endpoint, &auth, &host, &source, &sourcetype, &index, &mut buf).await;
                        }
                    }
                    None => {
                        if !buf.is_empty() {
                            flush(&client, &endpoint, &auth, &host, &source, &sourcetype, &index, &mut buf).await;
                        }
                        return;
                    }
                }
            }
            _ = ticker.tick() => {
                if !buf.is_empty() {
                    flush(&client, &endpoint, &auth, &host, &source, &sourcetype, &index, &mut buf).await;
                }
            }
        }
    }
}

async fn flush(
    client: &reqwest::Client,
    endpoint: &str,
    auth: &str,
    host: &str,
    source: &str,
    sourcetype: &str,
    index: &str,
    buf: &mut Vec<SplunkEvent>,
) {
    // HEC accepts newline-delimited JSON envelopes in a single POST body.
    let mut body = String::with_capacity(buf.len() * 256);
    for ev in buf.drain(..) {
        let envelope = json!({
            "time": ev.time,
            "host": host,
            "source": source,
            "sourcetype": sourcetype,
            "index": index,
            "event": {
                "request_id": ev.request_id,
                "method": ev.method,
                "path": ev.path,
                "status": ev.status,
                "duration_ms": ev.duration_ms,
                "service": "lynxx",
                "consumer": ev.consumer,
            }
        });
        body.push_str(&envelope.to_string());
        body.push('\n');
    }
    let res = client
        .post(endpoint)
        .header("Authorization", auth)
        .header("Content-Type", "application/json")
        .body(body)
        .send()
        .await;
    match res {
        Ok(r) if r.status().is_success() => {}
        Ok(r) => tracing::warn!(status = %r.status(), "splunk: HEC returned non-2xx"),
        Err(e) => tracing::warn!(error = %e, "splunk: send failed"),
    }
}

fn hostname() -> String {
    std::env::var("HOSTNAME").unwrap_or_else(|_| "lynxx".to_string())
}

#[async_trait]
impl Plugin for SplunkPlugin {
    fn name(&self) -> &str {
        "splunk"
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
        let _ = (&self.source, &self.sourcetype, &self.index);
        Ok(())
    }

    async fn on_response(&self, resp: &mut Response<BoxBody>) -> Result<(), PluginError> {
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

        let now_ms = chrono::Utc::now().timestamp_millis();
        let ev = SplunkEvent {
            time: now_ms as f64 / 1000.0,
            request_id,
            method,
            path,
            status: resp.status().as_u16(),
            duration_ms: (now_ms - start_ms).max(0),
            consumer,
        };
        if let Err(mpsc::error::TrySendError::Full(_)) = self.tx.try_send(ev) {
            tracing::debug!("splunk: channel full; dropping event");
        }
        Ok(())
    }
}
