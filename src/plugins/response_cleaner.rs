use crate::error::{GatewayError, PluginError};
use crate::plugins::{BoxBody, Plugin};
use async_trait::async_trait;
use axum::http::header::{HeaderName, HeaderValue, SET_COOKIE};
use axum::http::{Request, Response};
use std::str::FromStr;

/// Strip selected response headers and Set-Cookie entries before the
/// response is returned to the client. The Set-Cookie header may appear
/// multiple times; we iterate every value and drop only those whose
/// `name=` prefix matches one of `remove_cookies`.
pub struct ResponseCleanerPlugin {
    remove_headers: Vec<HeaderName>,
    remove_cookies: Vec<String>,
}

impl ResponseCleanerPlugin {
    pub fn from_config(cfg: &serde_json::Value) -> Result<Self, GatewayError> {
        let remove_headers = cfg
            .get("remove_response_headers")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str())
                    .filter_map(|s| HeaderName::from_str(s).ok())
                    .collect()
            })
            .unwrap_or_default();
        let remove_cookies = cfg
            .get("remove_cookies")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        Ok(Self {
            remove_headers,
            remove_cookies,
        })
    }
}

#[async_trait]
impl Plugin for ResponseCleanerPlugin {
    fn name(&self) -> &str {
        "response-cleaner"
    }

    async fn on_request(&self, _req: &mut Request<axum::body::Body>) -> Result<(), PluginError> {
        Ok(())
    }

    async fn on_response(&self, resp: &mut Response<BoxBody>) -> Result<(), PluginError> {
        for h in &self.remove_headers {
            resp.headers_mut().remove(h);
        }
        if !self.remove_cookies.is_empty() {
            // `headers_mut` doesn't have a "filter in place" for multi-valued
            // entries, so collect the kept ones, remove all, then re-insert.
            let kept: Vec<HeaderValue> = resp
                .headers()
                .get_all(SET_COOKIE)
                .iter()
                .filter(|v| {
                    let s = v.to_str().unwrap_or("");
                    let key = s.split('=').next().unwrap_or("").trim();
                    !self.remove_cookies.iter().any(|d| d == key)
                })
                .cloned()
                .collect();
            resp.headers_mut().remove(SET_COOKIE);
            for v in kept {
                resp.headers_mut().append(SET_COOKIE, v);
            }
        }
        Ok(())
    }
}
