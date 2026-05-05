use crate::error::{GatewayError, PluginError};
use crate::plugins::{BoxBody, Plugin};
use async_trait::async_trait;
use axum::http::{Request, Response};
use std::sync::atomic::{AtomicUsize, Ordering};

/// Process-wide ceiling on websocket frame sizes. Set by `from_config` and
/// read by the websocket bridge. v1 caveat: this is global rather than
/// per-route — wiring it through `proxy/websocket.rs` request extensions
/// would require touching the bridge in non-trivial ways. For now, the most
/// recently-loaded `WebsocketSizeLimit` plugin wins. Acceptable because in
/// practice operators set a single ceiling per gateway.
pub static WS_MAX_BYTES: AtomicUsize = AtomicUsize::new(usize::MAX);

#[derive(Clone, Copy, Debug)]
pub struct WsMaxBytes(pub usize);

pub struct WebsocketSizeLimitPlugin {
    max_bytes: usize,
}

impl WebsocketSizeLimitPlugin {
    pub fn from_config(cfg: &serde_json::Value) -> Result<Self, GatewayError> {
        let max_bytes = cfg
            .get("max_message_bytes")
            .and_then(|v| v.as_u64())
            .map(|n| n as usize)
            .unwrap_or(65_536);
        WS_MAX_BYTES.store(max_bytes, Ordering::Release);
        Ok(Self { max_bytes })
    }
}

#[async_trait]
impl Plugin for WebsocketSizeLimitPlugin {
    fn name(&self) -> &str {
        "websocket-size-limit"
    }

    async fn on_request(&self, req: &mut Request<axum::body::Body>) -> Result<(), PluginError> {
        // Tag the request so a future per-route bridge can read it without a
        // global. The current bridge ignores extensions, but this is cheap
        // and forward-compatible.
        req.extensions_mut().insert(WsMaxBytes(self.max_bytes));
        Ok(())
    }

    async fn on_response(&self, _resp: &mut Response<BoxBody>) -> Result<(), PluginError> {
        Ok(())
    }
}
