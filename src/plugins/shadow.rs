use crate::error::{GatewayError, PluginError};
use crate::plugins::{BoxBody, Plugin};
use async_trait::async_trait;
use axum::http::{HeaderMap, Method, Request, Response};
use hyper::body::Bytes;
use std::sync::OnceLock;
use std::time::Duration;

const MAX_BODY_BYTES: usize = 1_048_576;

static SHADOW_CLIENT: OnceLock<reqwest::Client> = OnceLock::new();

fn shadow_client() -> &'static reqwest::Client {
    SHADOW_CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .pool_max_idle_per_host(8)
            .build()
            .expect("failed to build shadow http client")
    })
}

/// Mirror requests to a secondary upstream. The proxy continues to talk to
/// the primary target as if this plugin weren't there; the shadow send is
/// fire-and-forget on a `tokio::spawn`. Bodies are only mirrored when
/// `include_body` is set and `Content-Length` < 1 MiB — anything larger is
/// dropped to avoid memory blowup on streaming uploads.
pub struct ShadowPlugin {
    shadow_url: String,
    include_body: bool,
}

impl ShadowPlugin {
    pub fn from_config(cfg: &serde_json::Value) -> Result<Self, GatewayError> {
        let shadow_url = cfg
            .get("shadow_url")
            .and_then(|v| v.as_str())
            .ok_or_else(|| GatewayError::BadRequest("shadow: `shadow_url` is required".into()))?
            .trim_end_matches('/')
            .to_string();
        let include_body = cfg
            .get("include_body")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        // `fire_and_forget` is documented but always true today; kept for forward compat.
        let _fire_and_forget = cfg
            .get("fire_and_forget")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        Ok(Self {
            shadow_url,
            include_body,
        })
    }
}

fn clone_headers(src: &HeaderMap) -> reqwest::header::HeaderMap {
    let mut out = reqwest::header::HeaderMap::with_capacity(src.len());
    for (k, v) in src.iter() {
        if let (Ok(name), Ok(value)) = (
            reqwest::header::HeaderName::from_bytes(k.as_str().as_bytes()),
            reqwest::header::HeaderValue::from_bytes(v.as_bytes()),
        ) {
            out.append(name, value);
        }
    }
    // Strip hop-by-hop headers that don't make sense for the mirror request.
    out.remove(reqwest::header::HOST);
    out.remove(reqwest::header::CONTENT_LENGTH);
    out
}

#[async_trait]
impl Plugin for ShadowPlugin {
    fn name(&self) -> &str {
        "shadow"
    }

    async fn on_request(&self, req: &mut Request<axum::body::Body>) -> Result<(), PluginError> {
        let path_and_query = req
            .uri()
            .path_and_query()
            .map(|p| p.as_str().to_string())
            .unwrap_or_else(|| req.uri().path().to_string());
        let url = format!("{}{}", self.shadow_url, path_and_query);
        let method = req.method().clone();
        let headers = clone_headers(req.headers());

        let body_bytes: Option<Bytes> = if self.include_body {
            // Only mirror bodies on methods that semantically carry one and
            // where Content-Length is small enough to buffer safely.
            let cl = req
                .headers()
                .get(axum::http::header::CONTENT_LENGTH)
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.parse::<usize>().ok())
                .unwrap_or(usize::MAX);
            if cl <= MAX_BODY_BYTES && body_methods(&method) {
                // Buffer the body, mirror it, and place the bytes back so the
                // primary path still has access. This costs a clone but keeps
                // the proxy contract intact.
                let body = std::mem::replace(req.body_mut(), axum::body::Body::empty());
                match axum::body::to_bytes(body, MAX_BODY_BYTES).await {
                    Ok(b) => {
                        *req.body_mut() = axum::body::Body::from(b.clone());
                        Some(b)
                    }
                    Err(e) => {
                        tracing::debug!(error = %e, "shadow: body read failed; skipping mirror");
                        // Body is gone — replace with empty so the proxy doesn't deadlock.
                        *req.body_mut() = axum::body::Body::empty();
                        None
                    }
                }
            } else {
                None
            }
        } else {
            None
        };

        tokio::spawn(async move {
            let client = shadow_client();
            let mut builder = client
                .request(reqwest_method(&method), &url)
                .headers(headers);
            if let Some(b) = body_bytes {
                builder = builder.body(b.to_vec());
            }
            match builder.send().await {
                Ok(_) => {
                    metrics::counter!("lynxx_shadow_total", "result" => "ok").increment(1);
                }
                Err(e) => {
                    metrics::counter!("lynxx_shadow_total", "result" => "fail").increment(1);
                    tracing::debug!(error = %e, "shadow: send failed");
                }
            }
        });

        Ok(())
    }

    async fn on_response(&self, _resp: &mut Response<BoxBody>) -> Result<(), PluginError> {
        Ok(())
    }
}

fn body_methods(m: &Method) -> bool {
    matches!(
        *m,
        Method::POST | Method::PUT | Method::PATCH | Method::DELETE
    )
}

fn reqwest_method(m: &Method) -> reqwest::Method {
    reqwest::Method::from_bytes(m.as_str().as_bytes()).unwrap_or(reqwest::Method::GET)
}
