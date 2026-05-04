use crate::error::{GatewayError, PluginError};
use crate::plugins::request_transformer::{apply_transforms, parse_header_list, parse_header_map};
use crate::plugins::{BoxBody, Plugin};
use async_trait::async_trait;
use axum::http::header::{HeaderName, HeaderValue};
use axum::http::{Request, Response};

pub struct ResponseTransformerPlugin {
    add: Vec<(HeaderName, HeaderValue)>,
    remove: Vec<HeaderName>,
    replace: Vec<(HeaderName, HeaderValue)>,
}

impl ResponseTransformerPlugin {
    pub fn from_config(cfg: &serde_json::Value) -> Result<Self, GatewayError> {
        Ok(Self {
            add: parse_header_map(cfg.get("add_headers"))?,
            remove: parse_header_list(cfg.get("remove_headers"))?,
            replace: parse_header_map(cfg.get("replace_headers"))?,
        })
    }
}

#[async_trait]
impl Plugin for ResponseTransformerPlugin {
    fn name(&self) -> &str {
        "response-transformer"
    }

    async fn on_request(&self, _req: &mut Request<axum::body::Body>) -> Result<(), PluginError> {
        Ok(())
    }

    async fn on_response(&self, resp: &mut Response<BoxBody>) -> Result<(), PluginError> {
        apply_transforms(resp.headers_mut(), &self.add, &self.remove, &self.replace);
        Ok(())
    }
}
