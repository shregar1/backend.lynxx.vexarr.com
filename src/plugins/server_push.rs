use crate::error::{GatewayError, PluginError};
use crate::plugins::{BoxBody, Plugin};
use async_trait::async_trait;
use axum::http::{Request, Response, header::HeaderValue};

/// HTTP/2 server push via `Link: <...>; rel=preload` headers.
///
/// Note: True HTTP/2 PUSH_PROMISE is deprecated in all major browsers
/// (Chrome 106+, Firefox 118+, Safari 17+). This plugin instead injects
/// `Link` headers with `rel=preload` and optional `as` hints, which
/// triggers 103 Early Hints in supporting servers and browser prefetch.
///
/// Config:
/// ```json
/// {
///   "links": [
///     { "path": "/style.css", "as": "style" },
///     { "path": "/app.js", "as": "script" },
///     { "path": "/logo.webp", "as": "image" }
///   ]
/// }
/// ```
pub struct ServerPushPlugin {
    links: Vec<PushLink>,
}

struct PushLink {
    header_value: String,
}

impl ServerPushPlugin {
    pub fn from_config(cfg: &serde_json::Value) -> Result<Self, GatewayError> {
        let mut links = Vec::new();
        if let Some(arr) = cfg.get("links").and_then(|v| v.as_array()) {
            for item in arr {
                let path = item
                    .get("path")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| {
                        GatewayError::BadRequest("server-push: each link needs `path`".into())
                    })?;
                let as_type = item.get("as").and_then(|v| v.as_str());
                let nopush = item
                    .get("nopush")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);

                let mut val = format!("<{path}>; rel=preload");
                if let Some(t) = as_type {
                    val.push_str(&format!("; as={t}"));
                }
                if nopush {
                    val.push_str("; nopush");
                }
                links.push(PushLink { header_value: val });
            }
        }
        Ok(Self { links })
    }
}

#[async_trait]
impl Plugin for ServerPushPlugin {
    fn name(&self) -> &str {
        "server-push"
    }

    async fn on_request(
        &self,
        _req: &mut Request<axum::body::Body>,
    ) -> Result<(), PluginError> {
        Ok(())
    }

    async fn on_response(&self, resp: &mut Response<BoxBody>) -> Result<(), PluginError> {
        // Only inject on HTML responses.
        let is_html = resp
            .headers()
            .get(axum::http::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(|ct| ct.contains("text/html"))
            .unwrap_or(false);

        if !is_html {
            return Ok(());
        }

        for link in &self.links {
            if let Ok(v) = HeaderValue::from_str(&link.header_value) {
                resp.headers_mut().append("link", v);
            }
        }
        Ok(())
    }
}
