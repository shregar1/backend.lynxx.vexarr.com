//! `openapi-mock` — return spec-driven mock responses without contacting upstream.
//!
//! Distinct from the existing `mocking` plugin: where `mocking` takes a
//! hand-coded `responses` array, this plugin reads the OpenAPI spec at
//! construction time and serves the spec's own response examples. Useful for:
//!
//! - Dev environments where the upstream isn't deployed yet but the spec is
//!   stable enough to develop clients against.
//! - Contract-test runs in CI: the gateway plays the role of the upstream and
//!   confirms client requests match what the spec advertises.
//!
//! The plugin runs in `on_request`. If the request method+path matches an
//! operation, we pick the first 2xx response, find its first
//! `examples` entry (or fall back to `example`), and short-circuit with that
//! body. If `passthrough_unmatched` is true (default), unmatched requests
//! flow through to upstream untouched; if false, we return 501 Not
//! Implemented so the contract-test harness fails loudly.

pub mod config;

use crate::error::{GatewayError, PluginError};
use crate::plugins::{BoxBody, Plugin};
use async_trait::async_trait;
use axum::http::{Request, Response};
use openapiv3::{OpenAPI, Operation, ReferenceOr, Response as OAResponse};
use std::time::Duration;

pub use config::OpenApiMockConfig;

pub struct OpenApiMockPlugin {
    cfg: OpenApiMockConfig,
    spec: OpenAPI,
}

impl OpenApiMockPlugin {
    pub fn from_config(cfg: &serde_json::Value) -> Result<Self, GatewayError> {
        let parsed: OpenApiMockConfig = serde_json::from_value(cfg.clone())
            .map_err(|e| GatewayError::BadRequest(format!("openapi-mock config: {e}")))?;
        let raw = config::load_spec_blocking(&parsed.spec_path)?;
        let spec: OpenAPI = parse_any(&raw)
            .map_err(|e| GatewayError::BadRequest(format!("openapi-mock parse: {e}")))?;
        Ok(Self { cfg: parsed, spec })
    }
}

/// Parse YAML or JSON — try JSON first since it's strictly faster; fall back
/// to YAML on `serde_json` failure.
fn parse_any(raw: &str) -> Result<OpenAPI, String> {
    if let Ok(v) = serde_json::from_str::<OpenAPI>(raw) {
        return Ok(v);
    }
    serde_yaml::from_str::<OpenAPI>(raw).map_err(|e| e.to_string())
}

/// Find the operation matching this request. Returns `(path_template, op)`.
/// The match logic is deliberately simple — we walk every path in the spec
/// and check segment-by-segment, treating `{param}` segments as wildcards.
fn match_operation<'a>(spec: &'a OpenAPI, method: &str, req_path: &str) -> Option<&'a Operation> {
    let req_segs: Vec<&str> = req_path.split('/').filter(|s| !s.is_empty()).collect();
    for (tmpl, item_ref) in &spec.paths.paths {
        let item = match item_ref {
            ReferenceOr::Item(it) => it,
            ReferenceOr::Reference { .. } => continue,
        };
        let tmpl_segs: Vec<&str> = tmpl.split('/').filter(|s| !s.is_empty()).collect();
        if tmpl_segs.len() != req_segs.len() {
            continue;
        }
        let mut ok = true;
        for (t, r) in tmpl_segs.iter().zip(req_segs.iter()) {
            if t.starts_with('{') && t.ends_with('}') {
                if r.is_empty() {
                    ok = false;
                    break;
                }
                continue;
            }
            if t != r {
                ok = false;
                break;
            }
        }
        if !ok {
            continue;
        }
        let op = match method.to_ascii_uppercase().as_str() {
            "GET" => item.get.as_ref(),
            "POST" => item.post.as_ref(),
            "PUT" => item.put.as_ref(),
            "DELETE" => item.delete.as_ref(),
            "PATCH" => item.patch.as_ref(),
            "HEAD" => item.head.as_ref(),
            "OPTIONS" => item.options.as_ref(),
            "TRACE" => item.trace.as_ref(),
            _ => None,
        };
        if let Some(op) = op {
            return Some(op);
        }
    }
    None
}

/// Walk the operation's `responses` and pull a 2xx body example. Returns
/// `(status, content_type, body)` so the caller can synthesise a response.
fn pick_example(op: &Operation, default_status: u16) -> Option<(u16, String, Vec<u8>)> {
    // Prefer the configured default status if it's in the spec, otherwise
    // pick the lowest 2xx, otherwise `default`.
    let mut candidates: Vec<(u16, &OAResponse)> = Vec::new();
    let mut default_resp: Option<&OAResponse> = None;
    for (code, resp_ref) in &op.responses.responses {
        let resp = match resp_ref {
            ReferenceOr::Item(r) => r,
            ReferenceOr::Reference { .. } => continue,
        };
        match code {
            openapiv3::StatusCode::Code(c) => {
                candidates.push((*c, resp));
            }
            openapiv3::StatusCode::Range(_) => {}
        }
    }
    if let Some(default_ref) = &op.responses.default {
        if let ReferenceOr::Item(r) = default_ref {
            default_resp = Some(r);
        }
    }

    // Prefer exact match with the default_status, then lowest 2xx, then default.
    let chosen: Option<(u16, &OAResponse)> = candidates
        .iter()
        .find(|(c, _)| *c == default_status)
        .copied()
        .or_else(|| {
            candidates
                .iter()
                .filter(|(c, _)| (200..300).contains(c))
                .min_by_key(|(c, _)| *c)
                .copied()
        })
        .or_else(|| default_resp.map(|r| (default_status, r)));

    let (status, resp) = chosen?;

    // Walk `content` map; prefer `application/json` then anything else.
    let content = if let Some(c) = resp.content.get("application/json") {
        Some(("application/json", c))
    } else {
        resp.content.iter().next().map(|(k, v)| (k.as_str(), v))
    };
    let (ct, media) = content?;

    // Pick `examples` first (richer), then `example`.
    if let Some((_name, ex_ref)) = media.examples.iter().next() {
        if let ReferenceOr::Item(ex) = ex_ref {
            if let Some(v) = &ex.value {
                return Some((status, ct.to_string(), serde_json::to_vec(v).ok()?));
            }
        }
    }
    if let Some(v) = &media.example {
        return Some((status, ct.to_string(), serde_json::to_vec(v).ok()?));
    }
    None
}

#[async_trait]
impl Plugin for OpenApiMockPlugin {
    fn name(&self) -> &str {
        "openapi-mock"
    }

    async fn on_request(&self, req: &mut Request<axum::body::Body>) -> Result<(), PluginError> {
        let method = req.method().as_str().to_string();
        let path = req.uri().path().to_string();

        let op = match match_operation(&self.spec, &method, &path) {
            Some(op) => op,
            None => {
                return if self.cfg.passthrough_unmatched {
                    Ok(())
                } else {
                    Err(PluginError::ShortCircuit {
                        status: 501,
                        headers: vec![("content-type".into(), "application/json".into())],
                        body: br#"{"error":"openapi-mock: no operation in spec matches this request"}"#.to_vec(),
                    })
                }
            }
        };

        let (status, content_type, body) = match pick_example(op, self.cfg.default_status) {
            Some(t) => t,
            None => {
                return if self.cfg.passthrough_unmatched {
                    Ok(())
                } else {
                    Err(PluginError::ShortCircuit {
                        status: 501,
                        headers: vec![("content-type".into(), "application/json".into())],
                        body: br#"{"error":"openapi-mock: matched operation has no example response"}"#.to_vec(),
                    })
                }
            }
        };

        if self.cfg.delay_ms > 0 {
            tokio::time::sleep(Duration::from_millis(self.cfg.delay_ms)).await;
        }

        Err(PluginError::ShortCircuit {
            status,
            headers: vec![("content-type".into(), content_type)],
            body,
        })
    }

    async fn on_response(&self, _resp: &mut Response<BoxBody>) -> Result<(), PluginError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tiny_spec() -> &'static str {
        r#"
openapi: 3.0.3
info:
  title: tiny
  version: '1.0'
paths:
  /hello/{name}:
    get:
      responses:
        '200':
          description: ok
          content:
            application/json:
              example:
                greeting: hi
"#
    }

    fn build_plugin(spec: &str) -> OpenApiMockPlugin {
        // Write to a tempfile and load — exercises the same path as production.
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("spec.yaml");
        std::fs::write(&p, spec).unwrap();
        let cfg = serde_json::json!({
            "spec_path": p.to_string_lossy(),
            "default_status": 200,
            "delay_ms": 0,
            "passthrough_unmatched": true,
        });
        let plugin = OpenApiMockPlugin::from_config(&cfg).unwrap();
        std::mem::forget(dir); // keep it alive for the test
        plugin
    }

    #[tokio::test]
    async fn returns_example_on_match() {
        let plugin = build_plugin(tiny_spec());
        let mut req = axum::http::Request::builder()
            .method("GET")
            .uri("/hello/alice")
            .body(axum::body::Body::empty())
            .unwrap();
        let res = plugin.on_request(&mut req).await;
        match res {
            Err(PluginError::ShortCircuit { status, body, .. }) => {
                assert_eq!(status, 200);
                let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
                assert_eq!(v["greeting"], "hi");
            }
            other => panic!("expected short-circuit, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn passes_through_when_unmatched() {
        let plugin = build_plugin(tiny_spec());
        let mut req = axum::http::Request::builder()
            .method("GET")
            .uri("/nonexistent")
            .body(axum::body::Body::empty())
            .unwrap();
        let res = plugin.on_request(&mut req).await;
        assert!(res.is_ok(), "should passthrough: {res:?}");
    }
}
