use crate::db::Repository;
use crate::error::{GatewayError, PluginError};
use crate::plugins::{BoxBody, Plugin};
use async_trait::async_trait;
use axum::http::header::HeaderValue;
use axum::http::{Request, Response};
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use std::sync::Arc;

/// HTTP Basic auth backed by the consumer table.
///
/// **Pragmatic v1 caveat:** there is no separate password column; we treat
/// the consumer's stored `api_key` (a random UUID generated at consumer
/// creation time) as the basic-auth password. This keeps us off a schema
/// migration. Replace with a real password-hash column if/when needed.
pub struct BasicAuthPlugin {
    repo: Option<Arc<dyn Repository>>,
    hide_credentials: bool,
}

impl BasicAuthPlugin {
    pub fn from_config(
        cfg: &serde_json::Value,
        repo: Option<Arc<dyn Repository>>,
    ) -> Result<Self, GatewayError> {
        let hide_credentials = cfg
            .get("hide_credentials")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        Ok(Self {
            repo,
            hide_credentials,
        })
    }

    fn unauthorized() -> PluginError {
        // The error is mapped to a 401 by GatewayError::IntoResponse. The
        // WWW-Authenticate header isn't carried through that mapping today;
        // clients can still infer Basic from the 401 + scheme docs.
        PluginError::Unauthorized(r#"Basic realm="lynxx""#.into())
    }
}

#[async_trait]
impl Plugin for BasicAuthPlugin {
    fn name(&self) -> &str {
        "basic-auth"
    }

    async fn on_request(&self, req: &mut Request<axum::body::Body>) -> Result<(), PluginError> {
        let raw = req
            .headers()
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .ok_or_else(Self::unauthorized)?;
        let encoded = raw.strip_prefix("Basic ").ok_or_else(Self::unauthorized)?;
        let decoded = B64
            .decode(encoded.trim())
            .map_err(|_| Self::unauthorized())?;
        let pair = std::str::from_utf8(&decoded).map_err(|_| Self::unauthorized())?;
        let (username, password) = pair.split_once(':').ok_or_else(Self::unauthorized)?;

        let repo = self
            .repo
            .as_ref()
            .ok_or_else(|| PluginError::Internal("basic-auth: no repository".into()))?;

        let consumers = repo
            .list_consumers()
            .await
            .map_err(|e| PluginError::Internal(format!("consumer lookup: {e}")))?;

        let matched = consumers
            .into_iter()
            .find(|c| c.username == username && c.api_key.as_deref() == Some(password));
        let consumer = matched.ok_or_else(Self::unauthorized)?;

        if let Ok(v) = HeaderValue::from_str(&consumer.username) {
            req.headers_mut().insert("x-consumer-username", v);
        }
        if let Ok(v) = HeaderValue::from_str(&consumer.id.to_string()) {
            req.headers_mut().insert("x-consumer-id", v);
        }
        if self.hide_credentials {
            req.headers_mut().remove("authorization");
        }
        Ok(())
    }

    async fn on_response(&self, _resp: &mut Response<BoxBody>) -> Result<(), PluginError> {
        Ok(())
    }
}
