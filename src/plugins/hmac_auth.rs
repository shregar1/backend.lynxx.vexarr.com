use crate::db::Repository;
use crate::error::{GatewayError, PluginError};
use crate::plugins::{BoxBody, Plugin};
use async_trait::async_trait;
use axum::http::header::HeaderValue;
use axum::http::{Request, Response};
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use chrono::{DateTime, Utc};
use hmac::{Hmac, Mac};
use http_body_util::BodyExt;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::Arc;

type HmacSha256 = Hmac<Sha256>;

/// Kong-style HMAC request signing. The `Authorization` header carries
/// `Hmac username="alice", algorithm="hmac-sha256",
/// headers="date request-line", signature="<b64>"`.
///
/// **v1 caveat:** the consumer's `api_key` field doubles as the HMAC
/// secret (same convention as basic-auth and key-auth). A real
/// deployment should add a separate `hmac_secret` column.
pub struct HmacAuthPlugin {
    repo: Option<Arc<dyn Repository>>,
    algorithms: Vec<String>,
    clock_skew: i64,
    validate_body: bool,
}

impl HmacAuthPlugin {
    pub fn from_config(
        cfg: &serde_json::Value,
        repo: Option<Arc<dyn Repository>>,
    ) -> Result<Self, GatewayError> {
        let algorithms = cfg
            .get("algorithms")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_ascii_lowercase()))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_else(|| vec!["hmac-sha256".into()]);
        if !algorithms.iter().all(|a| a == "hmac-sha256") {
            return Err(GatewayError::BadRequest(
                "hmac-auth: only hmac-sha256 supported in v1".into(),
            ));
        }
        let clock_skew = cfg
            .get("clock_skew_seconds")
            .and_then(|v| v.as_i64())
            .unwrap_or(300);
        let validate_body = cfg
            .get("validate_request_body")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        Ok(Self {
            repo,
            algorithms,
            clock_skew,
            validate_body,
        })
    }
}

fn unauthorized(msg: impl Into<String>) -> PluginError {
    PluginError::Unauthorized(msg.into())
}

/// Parse `key="value"` pairs out of `Hmac key="v", key2="v2"`. Bare
/// (unquoted) values are tolerated for resilience.
fn parse_auth_params(raw: &str) -> Option<HashMap<String, String>> {
    let body = raw
        .strip_prefix("Hmac ")
        .or_else(|| raw.strip_prefix("hmac "))?;
    let mut out = HashMap::new();
    for part in body.split(',') {
        let part = part.trim();
        let (k, v) = part.split_once('=')?;
        let k = k.trim().to_ascii_lowercase();
        let v = v.trim().trim_matches('"').to_string();
        out.insert(k, v);
    }
    Some(out)
}

fn canonicalize(
    headers_list: &[String],
    req: &Request<axum::body::Body>,
) -> Result<String, PluginError> {
    let mut lines = Vec::with_capacity(headers_list.len());
    for name in headers_list {
        let lower = name.to_ascii_lowercase();
        if lower == "request-line" {
            // Per Kong's convention: METHOD path?query HTTP/1.1
            let pq = req
                .uri()
                .path_and_query()
                .map(|p| p.as_str())
                .unwrap_or("/");
            lines.push(format!("{} {} HTTP/1.1", req.method(), pq));
        } else {
            let value = req
                .headers()
                .get(&lower)
                .and_then(|v| v.to_str().ok())
                .ok_or_else(|| unauthorized(format!("missing signed header: {name}")))?;
            lines.push(format!("{}: {}", lower, value));
        }
    }
    Ok(lines.join("\n"))
}

#[async_trait]
impl Plugin for HmacAuthPlugin {
    fn name(&self) -> &str {
        "hmac-auth"
    }

    async fn on_request(&self, req: &mut Request<axum::body::Body>) -> Result<(), PluginError> {
        let raw = req
            .headers()
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .ok_or_else(|| unauthorized("missing authorization header"))?
            .to_string();
        let params = parse_auth_params(&raw)
            .ok_or_else(|| unauthorized("malformed Hmac authorization header"))?;
        let username = params
            .get("username")
            .ok_or_else(|| unauthorized("missing username"))?;
        let algorithm = params
            .get("algorithm")
            .map(|s| s.to_ascii_lowercase())
            .unwrap_or_else(|| "hmac-sha256".into());
        if !self.algorithms.contains(&algorithm) {
            return Err(unauthorized(format!("algorithm {algorithm} not allowed")));
        }
        let signed_headers: Vec<String> = params
            .get("headers")
            .map(|s| s.split_whitespace().map(String::from).collect())
            .unwrap_or_else(|| vec!["date".into()]);
        let provided_sig = params
            .get("signature")
            .ok_or_else(|| unauthorized("missing signature"))?;
        let provided_sig = B64
            .decode(provided_sig)
            .map_err(|_| unauthorized("signature not valid base64"))?;

        // Clock-skew check on Date if present in the signed list.
        if signed_headers
            .iter()
            .any(|h| h.eq_ignore_ascii_case("date"))
        {
            let date_raw = req
                .headers()
                .get("date")
                .and_then(|v| v.to_str().ok())
                .ok_or_else(|| unauthorized("missing Date header"))?;
            let parsed = DateTime::parse_from_rfc2822(date_raw)
                .map_err(|_| unauthorized("invalid Date header"))?;
            let delta = (Utc::now() - parsed.with_timezone(&Utc))
                .num_seconds()
                .abs();
            if delta > self.clock_skew {
                return Err(unauthorized(format!("clock skew {delta}s exceeds limit")));
            }
        }

        let repo = self
            .repo
            .as_ref()
            .ok_or_else(|| PluginError::Internal("hmac-auth: no repository".into()))?;
        let consumers = repo
            .list_consumers()
            .await
            .map_err(|e| PluginError::Internal(format!("consumer lookup: {e}")))?;
        let consumer = consumers
            .into_iter()
            .find(|c| &c.username == username)
            .ok_or_else(|| unauthorized("unknown consumer"))?;
        let secret = consumer
            .api_key
            .as_deref()
            .ok_or_else(|| unauthorized("consumer has no secret"))?;

        let canon = canonicalize(&signed_headers, req)?;
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
            .map_err(|e| PluginError::Internal(format!("hmac key: {e}")))?;
        mac.update(canon.as_bytes());
        mac.verify_slice(&provided_sig)
            .map_err(|_| unauthorized("signature mismatch"))?;

        // Optional body integrity check via Digest header.
        if self.validate_body {
            // Body must be buffered to hash it; for non-empty bodies, read
            // and replace. Empty bodies skip per spec.
            let content_length = req
                .headers()
                .get("content-length")
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.parse::<u64>().ok())
                .unwrap_or(0);
            if content_length > 0 {
                let digest = req
                    .headers()
                    .get("digest")
                    .and_then(|v| v.to_str().ok())
                    .ok_or_else(|| unauthorized("missing Digest header for body"))?
                    .to_string();
                let expected_b64 = digest
                    .strip_prefix("SHA-256=")
                    .ok_or_else(|| unauthorized("Digest must use SHA-256"))?;
                let expected = B64
                    .decode(expected_b64)
                    .map_err(|_| unauthorized("Digest not valid base64"))?;

                let body = std::mem::replace(req.body_mut(), axum::body::Body::empty());
                let bytes = body
                    .collect()
                    .await
                    .map_err(|e| PluginError::Internal(format!("body collect: {e}")))?
                    .to_bytes();
                let mut h = Sha256::new();
                h.update(&bytes);
                let actual = h.finalize();
                if actual.as_slice() != expected.as_slice() {
                    return Err(unauthorized("body digest mismatch"));
                }
                *req.body_mut() = axum::body::Body::from(bytes);
            }
        }

        if let Ok(v) = HeaderValue::from_str(&consumer.username) {
            req.headers_mut().insert("x-consumer-username", v);
        }
        if let Ok(v) = HeaderValue::from_str(&consumer.id.to_string()) {
            req.headers_mut().insert("x-consumer-id", v);
        }
        Ok(())
    }

    async fn on_response(&self, _resp: &mut Response<BoxBody>) -> Result<(), PluginError> {
        Ok(())
    }
}
