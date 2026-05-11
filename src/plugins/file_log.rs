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
const HDR_HOST: &str = "x-lynxx-filelog-host";
const HDR_USER_AGENT: &str = "x-lynxx-filelog-ua";
const HDR_REFERER: &str = "x-lynxx-filelog-referer";
const HDR_CLIENT_IP: &str = "x-lynxx-filelog-client-ip";
const HDR_REQ_SIZE: &str = "x-lynxx-filelog-req-size";

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
    /// Custom format template, e.g. `"{method} {path} -> {status} ({duration_ms}ms)"`.
    /// When set, overrides `format` and `fields`.
    custom_template: Option<String>,
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
    host: String,
    user_agent: String,
    referer: String,
    client_ip: String,
    request_size: i64,
    response_size: i64,
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

        let custom_template = cfg
            .get("template")
            .and_then(|v| v.as_str())
            .map(String::from);

        let (tx, rx) = mpsc::channel::<LogLine>(4096);
        let target = PathBuf::from(path);
        let format_for_task = format;
        let fields_for_task = fields.clone();
        let tmpl_for_task = custom_template.clone();
        tokio::spawn(async move {
            writer_task(target, rx, format_for_task, fields_for_task, tmpl_for_task).await;
        });

        Ok(Self { tx, fields, format, custom_template })
    }
}

async fn writer_task(
    path: PathBuf,
    mut rx: mpsc::Receiver<LogLine>,
    format: Format,
    fields: Vec<String>,
    custom_template: Option<String>,
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
                        let s = render(&line, format, &fields, custom_template.as_deref());
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

/// Expand a template string like `"{method} {path} -> {status}"` with log line values.
fn expand_template(template: &str, line: &LogLine) -> String {
    template
        .replace("{timestamp}", &line.timestamp)
        .replace("{method}", &line.method)
        .replace("{path}", &line.path)
        .replace("{status}", &line.status.to_string())
        .replace("{duration_ms}", &line.duration_ms.to_string())
        .replace("{service}", &line.service)
        .replace("{consumer}", &line.consumer)
        .replace("{host}", &line.host)
        .replace("{user_agent}", &line.user_agent)
        .replace("{referer}", &line.referer)
        .replace("{client_ip}", &line.client_ip)
        .replace("{request_size}", &line.request_size.to_string())
        .replace("{response_size}", &line.response_size.to_string())
}

fn render(line: &LogLine, format: Format, fields: &[String], template: Option<&str>) -> String {
    if let Some(tmpl) = template {
        return expand_template(tmpl, line);
    }
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
                    "host" => json!(line.host),
                    "user_agent" => json!(line.user_agent),
                    "referer" => json!(line.referer),
                    "client_ip" => json!(line.client_ip),
                    "request_size" => json!(line.request_size),
                    "response_size" => json!(line.response_size),
                    _ => continue,
                };
                obj.insert(f.clone(), v);
            }
            serde_json::Value::Object(obj).to_string()
        }
        Format::Combined => {
            format!(
                "{} {} {} [{}] \"{} {}\" {} {}ms \"{}\" \"{}\"",
                line.client_ip.as_str().is_empty().then_some("-").unwrap_or(&line.client_ip),
                line.host,
                if line.consumer.is_empty() { "-" } else { &line.consumer },
                line.timestamp,
                line.method,
                line.path,
                line.status,
                line.duration_ms,
                line.referer,
                line.user_agent,
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
        let host = req
            .headers()
            .get("host")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("-")
            .to_string();
        let user_agent = req
            .headers()
            .get("user-agent")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("-")
            .to_string();
        let referer = req
            .headers()
            .get("referer")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("-")
            .to_string();
        let client_ip = req
            .headers()
            .get("x-real-ip")
            .or_else(|| req.headers().get("x-forwarded-for"))
            .and_then(|v| v.to_str().ok())
            .map(|s| s.split(',').next().unwrap_or(s).trim().to_string())
            .unwrap_or_default();
        let req_size = req
            .headers()
            .get("content-length")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.parse::<i64>().ok())
            .unwrap_or(0);

        let set = |req: &mut Request<axum::body::Body>, key: &'static str, val: &str| {
            if let Ok(v) = axum::http::HeaderValue::from_str(val) {
                req.headers_mut().insert(key, v);
            }
        };
        set(req, HDR_METHOD, &method);
        set(req, HDR_PATH, &path);
        set(req, HDR_START, &now_ms.to_string());
        set(req, HDR_HOST, &host);
        set(req, HDR_USER_AGENT, &user_agent);
        set(req, HDR_REFERER, &referer);
        set(req, HDR_CLIENT_IP, &client_ip);
        set(req, HDR_REQ_SIZE, &req_size.to_string());

        let _ = &self.fields;
        let _ = self.format;
        let _ = &self.custom_template;
        Ok(())
    }

    async fn on_response(&self, resp: &mut Response<BoxBody>) -> Result<(), PluginError> {
        let get_hdr = |resp: &Response<BoxBody>, key: &str| -> String {
            resp.headers()
                .get(key)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("")
                .to_string()
        };
        let method = get_hdr(resp, HDR_METHOD);
        let path = get_hdr(resp, HDR_PATH);
        let start_ms = resp
            .headers()
            .get(HDR_START)
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.parse::<i64>().ok())
            .unwrap_or_else(|| chrono::Utc::now().timestamp_millis());
        let consumer = get_hdr(resp, HDR_CONSUMER);
        let host = get_hdr(resp, HDR_HOST);
        let user_agent = get_hdr(resp, HDR_USER_AGENT);
        let referer = get_hdr(resp, HDR_REFERER);
        let client_ip = get_hdr(resp, HDR_CLIENT_IP);
        let request_size = resp
            .headers()
            .get(HDR_REQ_SIZE)
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.parse::<i64>().ok())
            .unwrap_or(0);
        let response_size = resp
            .headers()
            .get("content-length")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.parse::<i64>().ok())
            .unwrap_or(0);

        for key in [HDR_METHOD, HDR_PATH, HDR_START, HDR_HOST, HDR_USER_AGENT, HDR_REFERER, HDR_CLIENT_IP, HDR_REQ_SIZE] {
            resp.headers_mut().remove(key);
        }

        let now = chrono::Utc::now();
        let line = LogLine {
            timestamp: now.to_rfc3339(),
            method,
            path,
            status: resp.status().as_u16(),
            duration_ms: (now.timestamp_millis() - start_ms).max(0),
            service: "lynxx".into(),
            consumer,
            host,
            user_agent,
            referer,
            client_ip,
            request_size,
            response_size,
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
