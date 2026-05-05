use crate::error::{GatewayError, PluginError};
use crate::plugins::{BoxBody, Plugin};
use async_trait::async_trait;
use axum::http::{Request, Response};
use chrono::Utc;
use std::sync::Arc;
use tokio::io::AsyncWriteExt;
use tokio::net::{TcpStream, UdpSocket};
use tokio::sync::Mutex;

const HDR_METHOD: &str = "x-lynxx-syslog-method";
const HDR_PATH: &str = "x-lynxx-syslog-path";
const HDR_START: &str = "x-lynxx-syslog-start";

#[derive(Clone, Copy)]
enum Protocol {
    Udp,
    Tcp,
}

#[derive(Clone, Copy)]
enum Severity {
    Debug = 7,
    Info = 6,
    Notice = 5,
    Warning = 4,
    Error = 3,
    Crit = 2,
}

impl Severity {
    fn parse(s: &str) -> Severity {
        match s.to_ascii_lowercase().as_str() {
            "debug" => Severity::Debug,
            "notice" => Severity::Notice,
            "warning" | "warn" => Severity::Warning,
            "error" | "err" => Severity::Error,
            "crit" | "critical" => Severity::Crit,
            _ => Severity::Info,
        }
    }
}

fn facility_code(name: &str) -> u8 {
    match name.to_ascii_lowercase().as_str() {
        "kern" => 0,
        "user" => 1,
        "mail" => 2,
        "daemon" => 3,
        "auth" => 4,
        "syslog" => 5,
        "local0" => 16,
        "local1" => 17,
        "local2" => 18,
        "local3" => 19,
        "local4" => 20,
        "local5" => 21,
        "local6" => 22,
        "local7" => 23,
        _ => 16,
    }
}

enum Sender {
    Udp(Arc<UdpSocket>),
    Tcp(Arc<Mutex<Option<TcpStream>>>, String),
}

/// RFC 5424 syslog forwarder. Each request emits one structured line; the
/// severity is chosen by status class so noisy 5xx errors are easy to alert on.
pub struct SyslogPlugin {
    sender: Sender,
    facility: u8,
    success_sev: Severity,
    client_err_sev: Severity,
    server_err_sev: Severity,
}

impl SyslogPlugin {
    pub fn from_config(cfg: &serde_json::Value) -> Result<Self, GatewayError> {
        let host = cfg
            .get("host")
            .and_then(|v| v.as_str())
            .unwrap_or("127.0.0.1")
            .to_string();
        let port = cfg.get("port").and_then(|v| v.as_u64()).unwrap_or(514) as u16;
        let proto = match cfg.get("protocol").and_then(|v| v.as_str()).unwrap_or("udp") {
            "tcp" => Protocol::Tcp,
            _ => Protocol::Udp,
        };
        let facility = facility_code(
            cfg.get("facility")
                .and_then(|v| v.as_str())
                .unwrap_or("local0"),
        );
        let success_sev = Severity::parse(
            cfg.get("successful_severity")
                .or_else(|| cfg.get("severity"))
                .and_then(|v| v.as_str())
                .unwrap_or("info"),
        );
        let client_err_sev = Severity::parse(
            cfg.get("client_errors_severity")
                .and_then(|v| v.as_str())
                .unwrap_or("warning"),
        );
        let server_err_sev = Severity::parse(
            cfg.get("server_errors_severity")
                .and_then(|v| v.as_str())
                .unwrap_or("error"),
        );

        let target = format!("{host}:{port}");
        let sender = match proto {
            Protocol::Udp => {
                let socket = std::net::UdpSocket::bind("0.0.0.0:0")
                    .map_err(|e| GatewayError::ConfigError(format!("syslog bind: {e}")))?;
                socket.set_nonblocking(true).ok();
                socket
                    .connect(&target)
                    .map_err(|e| GatewayError::ConfigError(format!("syslog connect: {e}")))?;
                let s = UdpSocket::from_std(socket)
                    .map_err(|e| GatewayError::ConfigError(format!("syslog adopt: {e}")))?;
                Sender::Udp(Arc::new(s))
            }
            Protocol::Tcp => Sender::Tcp(Arc::new(Mutex::new(None)), target),
        };

        Ok(Self {
            sender,
            facility,
            success_sev,
            client_err_sev,
            server_err_sev,
        })
    }

    fn pri(&self, sev: Severity) -> u8 {
        self.facility * 8 + sev as u8
    }

    fn pick_severity(&self, status: u16) -> Severity {
        match status {
            500..=599 => self.server_err_sev,
            400..=499 => self.client_err_sev,
            _ => self.success_sev,
        }
    }

    async fn send(&self, line: String) {
        match &self.sender {
            Sender::Udp(s) => {
                if let Err(e) = s.send(line.as_bytes()).await {
                    tracing::debug!(error = %e, "syslog udp send failed");
                }
            }
            Sender::Tcp(slot, target) => {
                let mut guard = slot.lock().await;
                let needs_connect = guard.is_none();
                if needs_connect {
                    match TcpStream::connect(target).await {
                        Ok(s) => *guard = Some(s),
                        Err(e) => {
                            tracing::debug!(error = %e, "syslog tcp connect failed");
                            return;
                        }
                    }
                }
                let stream = match guard.as_mut() {
                    Some(s) => s,
                    None => return,
                };
                // RFC 6587 octet-counted framing keeps things simple over TCP.
                let framed = format!("{} {}\n", line.len(), line);
                if let Err(e) = stream.write_all(framed.as_bytes()).await {
                    tracing::debug!(error = %e, "syslog tcp write failed; will reconnect");
                    *guard = None;
                }
            }
        }
    }
}

#[async_trait]
impl Plugin for SyslogPlugin {
    fn name(&self) -> &str {
        "syslog"
    }

    async fn on_request(&self, req: &mut Request<axum::body::Body>) -> Result<(), PluginError> {
        let method = req.method().as_str().to_string();
        let path = req.uri().path().to_string();
        if let Ok(v) = axum::http::HeaderValue::from_str(&method) {
            req.headers_mut().insert(HDR_METHOD, v);
        }
        if let Ok(v) = axum::http::HeaderValue::from_str(&path) {
            req.headers_mut().insert(HDR_PATH, v);
        }
        let start = std::time::Instant::now().elapsed().as_nanos() as u64;
        let _ = start;
        let now_ms = chrono::Utc::now().timestamp_millis();
        if let Ok(v) = axum::http::HeaderValue::from_str(&now_ms.to_string()) {
            req.headers_mut().insert(HDR_START, v);
        }
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
        resp.headers_mut().remove(HDR_METHOD);
        resp.headers_mut().remove(HDR_PATH);
        resp.headers_mut().remove(HDR_START);

        let status = resp.status().as_u16();
        let now_ms = chrono::Utc::now().timestamp_millis();
        let elapsed_ms = (now_ms - start_ms).max(0);

        let pri = self.pri(self.pick_severity(status));
        let timestamp = Utc::now().to_rfc3339();
        let hostname = "lynxx";
        let app = "lynxx-gateway";
        // RFC 5424: <PRI>VERSION TIMESTAMP HOSTNAME APP-NAME PROCID MSGID STRUCTURED-DATA MSG
        let line = format!(
            "<{pri}>1 {timestamp} {hostname} {app} - - - {method} {path} {status} {elapsed_ms}ms"
        );
        self.send(line).await;
        Ok(())
    }
}
