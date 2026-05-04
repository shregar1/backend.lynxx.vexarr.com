pub mod auth;
pub mod cors;
pub mod logging;
pub mod rate_limit;

use crate::error::PluginError;
use async_trait::async_trait;
use axum::http::{Request, Response};
use hyper::body::Bytes;
use std::sync::Arc;

pub type BoxBody = http_body_util::Full<Bytes>;

#[async_trait]
pub trait Plugin: Send + Sync {
    fn name(&self) -> &str;
    async fn on_request(
        &self,
        req: &mut Request<axum::body::Body>,
    ) -> Result<(), PluginError>;
    async fn on_response(
        &self,
        resp: &mut Response<BoxBody>,
    ) -> Result<(), PluginError>;
}

pub struct PluginRegistry {
    plugins: Vec<Arc<dyn Plugin>>,
}

impl PluginRegistry {
    pub fn new() -> Self {
        Self {
            plugins: Vec::new(),
        }
    }

    pub fn register(&mut self, plugin: Arc<dyn Plugin>) {
        tracing::info!("registered plugin: {}", plugin.name());
        self.plugins.push(plugin);
    }

    pub fn plugins(&self) -> &[Arc<dyn Plugin>] {
        &self.plugins
    }
}
