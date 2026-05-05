use crate::error::{GatewayError, PluginError};
use crate::plugins::{BoxBody, Plugin};
use async_trait::async_trait;
use axum::http::header::{HeaderName, HeaderValue};
use axum::http::{Request, Response};
use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};
use uuid::Uuid;

/// Internal header used to carry the chosen request id from `on_request` to
/// `on_response` so we can echo it back without re-parsing the inbound header
/// (which may have been mutated by other plugins).
const HDR_PINNED: &str = "x-lynxx-rid-pin";

#[derive(Clone, Copy)]
enum Generator {
    Uuid,
    Nanoid,
    Sequential,
}

/// Request-id stamper. Honors an inbound id (so the chain is idempotent) or
/// generates a fresh one. Echoes the value to the client when `echo_downstream`
/// is set.
pub struct RequestIdPlugin {
    header_name: HeaderName,
    echo_downstream: bool,
    generator: Generator,
    counter: AtomicU64,
}

impl RequestIdPlugin {
    pub fn from_config(cfg: &serde_json::Value) -> Result<Self, GatewayError> {
        let header = cfg
            .get("header_name")
            .and_then(|v| v.as_str())
            .unwrap_or("X-Request-Id");
        let header_name = HeaderName::from_str(header)
            .map_err(|e| GatewayError::BadRequest(format!("invalid header name: {e}")))?;
        let echo_downstream = cfg
            .get("echo_downstream")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        let generator = match cfg.get("generator").and_then(|v| v.as_str()).unwrap_or("uuid") {
            "nanoid" => Generator::Nanoid,
            "sequential" => Generator::Sequential,
            _ => Generator::Uuid,
        };
        Ok(Self {
            header_name,
            echo_downstream,
            generator,
            counter: AtomicU64::new(1),
        })
    }

    fn generate(&self) -> String {
        match self.generator {
            Generator::Uuid => Uuid::new_v4().to_string(),
            Generator::Nanoid => {
                // 21-char URL-safe id without pulling the `nanoid` crate.
                const ALPHABET: &[u8] =
                    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
                let seed = Uuid::new_v4().as_u128();
                let mut s = seed as u64;
                let mut out = String::with_capacity(21);
                for _ in 0..21 {
                    s ^= s << 13;
                    s ^= s >> 7;
                    s ^= s << 17;
                    out.push(ALPHABET[(s as usize) % ALPHABET.len()] as char);
                }
                out
            }
            Generator::Sequential => self
                .counter
                .fetch_add(1, Ordering::Relaxed)
                .to_string(),
        }
    }
}

#[async_trait]
impl Plugin for RequestIdPlugin {
    fn name(&self) -> &str {
        "request-id"
    }

    async fn on_request(&self, req: &mut Request<axum::body::Body>) -> Result<(), PluginError> {
        let existing = req
            .headers()
            .get(&self.header_name)
            .and_then(|v| v.to_str().ok())
            .map(String::from);
        let id = existing.unwrap_or_else(|| self.generate());
        if let Ok(v) = HeaderValue::from_str(&id) {
            req.headers_mut().insert(self.header_name.clone(), v.clone());
            req.headers_mut().insert(HDR_PINNED, v);
        }
        // Best-effort: also stash on the request extensions so other plugins
        // (and the proxy handler) can read it without reaching for headers.
        req.extensions_mut().insert(RequestId(id));
        Ok(())
    }

    async fn on_response(&self, resp: &mut Response<BoxBody>) -> Result<(), PluginError> {
        if !self.echo_downstream {
            resp.headers_mut().remove(HDR_PINNED);
            return Ok(());
        }
        if let Some(v) = resp.headers().get(HDR_PINNED).cloned() {
            resp.headers_mut().insert(self.header_name.clone(), v);
        }
        resp.headers_mut().remove(HDR_PINNED);
        Ok(())
    }
}

/// Request-extension wrapper so other plugins can `req.extensions().get::<RequestId>()`.
#[derive(Clone, Debug)]
pub struct RequestId(pub String);
