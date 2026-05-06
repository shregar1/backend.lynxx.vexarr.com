use crate::error::{GatewayError, PluginError};
use crate::plugins::{BoxBody, Plugin};
use async_trait::async_trait;
use axum::http::header::{HeaderName, HeaderValue};
use axum::http::{Request, Response};
use std::str::FromStr;
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

const HDR_PINNED: &str = "x-lynxx-cid-pin";

/// W3C Trace Context correlation id. Honors an inbound `traceparent`
/// matching `00-<32hex>-<16hex>-<2hex>`, or synthesizes one. Distinct from
/// `request-id` because OTel/Jaeger consumers reject malformed traceparents,
/// and free-form ids are useful for human log correlation outside tracing.
pub struct CorrelationIdPlugin {
    header_name: HeaderName,
}

impl CorrelationIdPlugin {
    pub fn from_config(cfg: &serde_json::Value) -> Result<Self, GatewayError> {
        let header = cfg
            .get("header_name")
            .and_then(|v| v.as_str())
            .unwrap_or("traceparent");
        let header_name = HeaderName::from_str(header)
            .map_err(|e| GatewayError::BadRequest(format!("invalid header name: {e}")))?;
        // `generator` is currently always w3c; accept the field for forward compat.
        let _gen = cfg
            .get("generator")
            .and_then(|v| v.as_str())
            .unwrap_or("w3c");
        Ok(Self { header_name })
    }

    fn valid_traceparent(s: &str) -> bool {
        let parts: Vec<&str> = s.split('-').collect();
        if parts.len() != 4 {
            return false;
        }
        let lens = [2, 32, 16, 2];
        for (p, n) in parts.iter().zip(lens.iter()) {
            if p.len() != *n || !p.chars().all(|c| c.is_ascii_hexdigit()) {
                return false;
            }
        }
        true
    }

    fn synthesize() -> String {
        let trace = format!("{:032x}", Uuid::new_v4().as_u128());
        let span_seed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0)
            ^ Uuid::new_v4().as_u128() as u64;
        format!("00-{trace}-{:016x}-01", span_seed)
    }
}

#[async_trait]
impl Plugin for CorrelationIdPlugin {
    fn name(&self) -> &str {
        "correlation-id"
    }

    async fn on_request(&self, req: &mut Request<axum::body::Body>) -> Result<(), PluginError> {
        let incoming = req
            .headers()
            .get(&self.header_name)
            .and_then(|v| v.to_str().ok())
            .map(String::from);
        let value = match incoming {
            Some(s) if Self::valid_traceparent(&s) => s,
            _ => Self::synthesize(),
        };
        if let Ok(v) = HeaderValue::from_str(&value) {
            req.headers_mut()
                .insert(self.header_name.clone(), v.clone());
            req.headers_mut().insert(HDR_PINNED, v);
        }
        req.extensions_mut().insert(CorrelationId(value));
        Ok(())
    }

    async fn on_response(&self, resp: &mut Response<BoxBody>) -> Result<(), PluginError> {
        if let Some(v) = resp.headers().get(HDR_PINNED).cloned() {
            resp.headers_mut().insert(self.header_name.clone(), v);
        }
        resp.headers_mut().remove(HDR_PINNED);
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct CorrelationId(pub String);
