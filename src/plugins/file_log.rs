use crate::error::{GatewayError, PluginError};
use crate::plugins::{BoxBody, Plugin};
use async_trait::async_trait;
use axum::http::{Request, Response};
use serde_json::json;
use std::path::PathBuf;
use std::time::Duration;
use tokio::fs::OpenOptions;
use tokio::io::AsyncWriteExt;
use tokio::sync::mpsc;

const HDR_METHOD: &str = "x-lynxx-filelog-method";
const HDR_PATH: &str = "x-lynxx-filelog-path";
const HDR_START: &str = "x-lynxx-filelog-start";
const HDR_CONSUMER: &str = "x-consumer-username";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Format {
    Json,
    Combined,
}

/// Per-request access logger. The hot path only `try_send`s a small struct
/// onto a bounded mpsc; an owned background task handles file I/O so a slow
/// disk never stalls request processing. The writer batches lines and flushes
/// every 100ms or 256 entries, whichever comes first.
pub struct FileLogPlugin {
    tx: mpsc::Sender<LogLine>,
    fields: Vec<String>,
    format: Format,
}

#[derive(Debug)]
struct LogLine {
    timestamp: String,
    method: String,
    path: String,
    status: u16,
    duration_ms: i64,
    service: String,
    consumer: String,
}

impl FileLogPlugin {
    pub fn from_config(cfg: &serde_json::Value) -> Result<Self, GatewayError> {
        let path = cfg
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| GatewayError::BadRequest("file-log: `path` is required".into()))?
            .to_string();
        let format = match cfg.get("format").and_then(|v| v.as_str()).unwrap_or("json") {
            "combined" => Format::Combined,
            _ => Format::Json,
        };
        let fields = cfg
            .get("fields")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_else(|| {
                vec![
                    "timestamp".into(),
                    "method".into(),
                    "path".into(),
                    "status".into(),
                    "duration_ms".into(),
                    "service".into(),
                    "consumer".into(),
                ]
            });

        let (tx, rx) = mpsc::channel::<LogLine>(4096);
        let target = PathBuf::from(path);
        let format_for_task = format;
        let fields_for_task = fields.clone();
        // Lives until process exit. Plugin updates create a fresh instance
        // (and thus a fresh task) so leaked tasks per rebuild are bounded by
        // process lifetime, which is acceptable for v1.
        tokio::spawn(async move {
            writer_task(target, rx, format_for_task, fields_for_task).await;
        });

        Ok(Self {
            tx,
            fields,
            format,
        })
    }
}

async fn writer_task(
    path: PathBuf,
    mut rx: mpsc::Receiver<LogLine>,
    format: Format,
    fields: Vec<String>,
) {
    let file = match OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .await
    {
        Ok(f) => f,
        Err(e) => {
            tracing::warn!(path = %path.display(), error = %e, "file-log: open failed; logs will be dropped");
            return;
        }
    };
    let mut file = file;
    let mut buf: Vec<u8> = Vec::with_capacity(8 * 1024);
    let mut count: usize = 0;
    let flush_interval = Duration::from_millis(100);
    let mut ticker = tokio::time::interval(flush_interval);

    loop {
        tokio::select! {
            biased;
            maybe = rx.recv() => {
                match maybe {
                    Some(line) => {
                        let s = render(&line, format, &fields);
                        buf.extend_from_slice(s.as_bytes());
                        buf.push(b'\n');
                        count += 1;
                        if count >= 256 {
                            let _ = file.write_all(&buf).await;
                            buf.clear();
                            count = 0;
                        }
                    }
                    None => {
                        let _ = file.write_all(&buf).await;
                        let _ = file.flush().await;
                        return;
                    }
                }
            }
            _ = ticker.tick() => {
                if !buf.is_empty() {
                    let _ = file.write_all(&buf).await;
                    let _ = file.flush().await;
                    buf.clear();
                    count = 0;
                }
            }
        }
    }
}

fn render(line: &LogLine, format: Format, fields: &[String]) -> String {
    match format {
        Format::Json => {
            let mut obj = serde_json::Map::new();
            for f in fields {
                let v = match f.as_str() {
                    "timestamp" => json!(line.timestamp),
                    "method" => json!(line.method),
                    "path" => json!(line.path),
                    "status" => json!(line.status),
                    "duration_ms" => json!(line.duration_ms),
                    "service" => json!(line.service),
                    "consumer" => json!(line.consumer),
                    _ => continue,
                };
                obj.insert(f.clone(), v);
            }
            serde_json::Value::Object(obj).to_string()
        }
        Format::Combined => {
            // Apache combined-log-ish: host - user [time] "METH path" status -
            format!(
                "{} - {} [{}] \"{} {}\" {} {}ms",
                line.service,
                if line.consumer.is_empty() { "-" } else { &line.consumer },
                line.timestamp,
                line.method,
                line.path,
                line.status,
                line.duration_ms,
            )
        }
    }
}

#[async_trait]
impl Plugin for FileLogPlugin {
    fn name(&self) -> &str {
        "file-log"
    }

    async fn on_request(&self, req: &mut Request<axum::body::Body>) -> Result<(), PluginError> {
        let method = req.method().as_str().to_string();
        let path = req.uri().path().to_string();
        let now_ms = chrono::Utc::now().timestamp_millis();
        if let Ok(v) = axum::http::HeaderValue::from_str(&method) {
            req.headers_mut().insert(HDR_METHOD, v);
        }
        if let Ok(v) = axum::http::HeaderValue::from_str(&path) {
            req.headers_mut().insert(HDR_PATH, v);
        }
        if let Ok(v) = axum::http::HeaderValue::from_str(&now_ms.to_string()) {
            req.headers_mut().insert(HDR_START, v);
        }
        let _ = &self.fields; // reserved for future per-request field selection
        let _ = self.format;
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
        let consumer = resp
            .headers()
            .get(HDR_CONSUMER)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        resp.headers_mut().remove(HDR_METHOD);
        resp.headers_mut().remove(HDR_PATH);
        resp.headers_mut().remove(HDR_START);

        let now = chrono::Utc::now();
        let line = LogLine {
            timestamp: now.to_rfc3339(),
            method,
            path,
            status: resp.status().as_u16(),
            duration_ms: (now.timestamp_millis() - start_ms).max(0),
            service: "lynxx".into(),
            consumer,
        };
        // Drop on full to avoid blocking the response path. A full channel
        // means the writer is severely behind — letting the request return is
        // strictly better than back-pressuring the proxy.
        if let Err(mpsc::error::TrySendError::Full(_)) = self.tx.try_send(line) {
            tracing::debug!("file-log channel full; dropping line");
        }
        Ok(())
    }
}
