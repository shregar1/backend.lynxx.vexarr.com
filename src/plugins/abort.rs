//! Abort plugin — silently close the connection without sending a response.
//!
//! ## Config
//!
//! ```json
//! { "message": "connection refused" }
//! ```
//!
//! Useful for honeypot routes or blocking abusive clients. The upstream
//! never receives the request; the client sees a connection reset.

use crate::error::{GatewayError, PluginError};
use crate::plugins::{BoxBody, Plugin};
use axum::body::Body;
use axum::http::{Request, Response};

pub struct AbortPlugin {
    message: String,
}

impl AbortPlugin {
    pub fn from_config(config: &serde_json::Value) -> Result<Self, GatewayError> {
        let message = config["message"]
            .as_str()
            .unwrap_or("connection aborted")
            .to_string();
        Ok(Self { message })
    }
}

#[async_trait::async_trait]
impl Plugin for AbortPlugin {
    fn name(&self) -> &'static str {
        "abort"
    }

    async fn on_request(&self, _req: &mut Request<Body>) -> Result<(), PluginError> {
        // Return an error that the handler converts into a connection close.
        // StatusCode 444 (nginx-style "No Response") signals the handler to
        // drop the connection without sending any HTTP response.
        Err(PluginError::Internal(format!("abort: {}", self.message)))
    }

    async fn on_response(&self, _resp: &mut Response<BoxBody>) -> Result<(), PluginError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_message() {
        let cfg = serde_json::json!({});
        let plugin = AbortPlugin::from_config(&cfg).unwrap();
        assert_eq!(plugin.message, "connection aborted");
    }

    #[test]
    fn custom_message() {
        let cfg = serde_json::json!({"message": "go away"});
        let plugin = AbortPlugin::from_config(&cfg).unwrap();
        assert_eq!(plugin.message, "go away");
    }
}
