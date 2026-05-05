use crate::error::{GatewayError, PluginError};
use crate::plugins::{BoxBody, Plugin};
use async_trait::async_trait;
use axum::http::header::{HeaderName, HeaderValue, COOKIE};
use axum::http::{Request, Response, Uri};
use std::str::FromStr;

/// Strip selected request headers, query parameters, and cookies before the
/// request is forwarded upstream. Useful for stripping internal-only headers,
/// debug query flags, or session cookies that an upstream shouldn't see.
pub struct RequestCleanerPlugin {
    remove_headers: Vec<HeaderName>,
    remove_query: Vec<String>,
    remove_cookies: Vec<String>,
}

impl RequestCleanerPlugin {
    pub fn from_config(cfg: &serde_json::Value) -> Result<Self, GatewayError> {
        let remove_headers = cfg
            .get("remove_request_headers")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str())
                    .filter_map(|s| HeaderName::from_str(s).ok())
                    .collect()
            })
            .unwrap_or_default();
        let remove_query = cfg
            .get("remove_query_params")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
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
            remove_query,
            remove_cookies,
        })
    }

    fn rebuild_uri_without(uri: &Uri, drop: &[String]) -> Option<Uri> {
        let q = uri.query()?;
        // url::Url needs a base; we only care about the query semantics.
        let dummy = format!("http://x{}?{}", uri.path(), q);
        let parsed = url::Url::parse(&dummy).ok()?;
        let kept: Vec<(String, String)> = parsed
            .query_pairs()
            .filter(|(k, _)| !drop.iter().any(|d| d == k.as_ref()))
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect();
        let mut path_and_query = uri.path().to_string();
        if !kept.is_empty() {
            let qs: String = url::form_urlencoded::Serializer::new(String::new())
                .extend_pairs(kept.iter().map(|(k, v)| (k.as_str(), v.as_str())))
                .finish();
            path_and_query.push('?');
            path_and_query.push_str(&qs);
        }
        let mut parts = uri.clone().into_parts();
        parts.path_and_query = Some(path_and_query.parse().ok()?);
        Uri::from_parts(parts).ok()
    }

    fn strip_cookies(value: &str, drop: &[String]) -> String {
        value
            .split(';')
            .map(|p| p.trim())
            .filter(|p| {
                let key = p.split('=').next().unwrap_or("").trim();
                !drop.iter().any(|d| d == key)
            })
            .collect::<Vec<_>>()
            .join("; ")
    }
}

#[async_trait]
impl Plugin for RequestCleanerPlugin {
    fn name(&self) -> &str {
        "request-cleaner"
    }

    async fn on_request(&self, req: &mut Request<axum::body::Body>) -> Result<(), PluginError> {
        for h in &self.remove_headers {
            req.headers_mut().remove(h);
        }
        if !self.remove_query.is_empty() && req.uri().query().is_some() {
            if let Some(new_uri) = Self::rebuild_uri_without(req.uri(), &self.remove_query) {
                *req.uri_mut() = new_uri;
            }
        }
        if !self.remove_cookies.is_empty() {
            if let Some(cookie) = req.headers().get(COOKIE).cloned() {
                if let Ok(s) = cookie.to_str() {
                    let cleaned = Self::strip_cookies(s, &self.remove_cookies);
                    if cleaned.is_empty() {
                        req.headers_mut().remove(COOKIE);
                    } else if let Ok(v) = HeaderValue::from_str(&cleaned) {
                        req.headers_mut().insert(COOKIE, v);
                    }
                }
            }
        }
        Ok(())
    }

    async fn on_response(&self, _resp: &mut Response<BoxBody>) -> Result<(), PluginError> {
        Ok(())
    }
}
