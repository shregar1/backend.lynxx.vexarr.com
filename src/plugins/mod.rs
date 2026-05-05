pub mod acl;
pub mod auth;
pub mod basic_auth;
pub mod bot_detection;
pub mod cors;
pub mod factory;
pub mod hmac_auth;
pub mod ip_restriction;
pub mod jwt;
pub mod key_auth;
pub mod logging;
pub mod oauth2_introspect;
pub mod proxy_cache;
pub mod rate_limit;
pub mod request_size_limit;
pub mod request_termination;
pub mod request_transformer;
pub mod request_validator;
pub mod response_transformer;
pub mod session;

use crate::db::Repository;
use crate::error::{GatewayError, PluginError};
use async_trait::async_trait;
use axum::http::{Request, Response};
use hyper::body::Bytes;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::RwLock;
use uuid::Uuid;

pub type BoxBody = http_body_util::Full<Bytes>;

#[async_trait]
pub trait Plugin: Send + Sync {
    fn name(&self) -> &str;
    async fn on_request(&self, req: &mut Request<axum::body::Body>) -> Result<(), PluginError>;
    async fn on_response(&self, resp: &mut Response<BoxBody>) -> Result<(), PluginError>;
}

/// Index of which plugin instance ids apply at each scope.
#[derive(Default)]
pub struct PluginIndex {
    pub global: Vec<Uuid>,
    pub by_service: HashMap<Uuid, Vec<Uuid>>,
    pub by_route: HashMap<Uuid, Vec<Uuid>>,
}

/// Dynamic plugin registry. Plugins are persisted in the DB and rebuilt into
/// concrete `Arc<dyn Plugin>` instances cached here. Every plugin CRUD op in
/// the admin API triggers `rebuild` to refresh the cache.
pub struct PluginExecutor {
    built: RwLock<HashMap<Uuid, Arc<dyn Plugin>>>,
    index: RwLock<PluginIndex>,
    /// Fast-path flag: `true` when no plugins are configured anywhere. Lets the
    /// hot path skip the `RwLock` reads entirely. Updated atomically on every
    /// `rebuild`. When this is set, both `run_request` and `run_response`
    /// return without acquiring any lock — the common production-default case
    /// when a route has zero scoped plugins.
    is_empty: AtomicBool,
}

impl PluginExecutor {
    pub fn new() -> Self {
        Self {
            built: RwLock::new(HashMap::new()),
            index: RwLock::new(PluginIndex::default()),
            is_empty: AtomicBool::new(true),
        }
    }

    /// Lock-free check used by the proxy hot path to skip plugin dispatch
    /// entirely when no plugin is configured.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.is_empty.load(Ordering::Acquire)
    }

    /// Re-fetch all plugin rows from the DB and (re)build every enabled plugin.
    /// Disabled plugins are skipped. Build failures are logged but do not abort
    /// the rebuild — other plugins still load.
    pub async fn rebuild(&self, repo: &Arc<dyn Repository>) -> Result<(), GatewayError> {
        let rows = repo.list_plugins().await?;
        let mut built: HashMap<Uuid, Arc<dyn Plugin>> = HashMap::with_capacity(rows.len());
        let mut index = PluginIndex::default();

        for row in rows {
            if !row.enabled {
                continue;
            }
            let plugin = match factory::build(&row.name, &row.config, Some(repo.clone())) {
                Ok(p) => p,
                Err(e) => {
                    tracing::warn!(
                        plugin = %row.name,
                        id = %row.id,
                        error = %e,
                        "failed to build plugin; skipping"
                    );
                    continue;
                }
            };

            match (row.service_id, row.route_id) {
                (_, Some(route_id)) => {
                    index.by_route.entry(route_id).or_default().push(row.id);
                }
                (Some(service_id), None) => {
                    index.by_service.entry(service_id).or_default().push(row.id);
                }
                (None, None) => {
                    index.global.push(row.id);
                }
            }

            built.insert(row.id, plugin);
        }

        let empty = built.is_empty()
            && index.global.is_empty()
            && index.by_service.is_empty()
            && index.by_route.is_empty();
        *self.built.write().await = built;
        *self.index.write().await = index;
        self.is_empty.store(empty, Ordering::Release);
        Ok(())
    }

    /// Resolve the request-time plugin chain: global → service → route.
    async fn resolve_chain(
        &self,
        service_id: Option<Uuid>,
        route_id: Option<Uuid>,
    ) -> Vec<Arc<dyn Plugin>> {
        let index = self.index.read().await;
        let built = self.built.read().await;

        let mut chain: Vec<Arc<dyn Plugin>> = Vec::new();
        for id in &index.global {
            if let Some(p) = built.get(id) {
                chain.push(p.clone());
            }
        }
        if let Some(svc) = service_id {
            if let Some(ids) = index.by_service.get(&svc) {
                for id in ids {
                    if let Some(p) = built.get(id) {
                        chain.push(p.clone());
                    }
                }
            }
        }
        if let Some(rt) = route_id {
            if let Some(ids) = index.by_route.get(&rt) {
                for id in ids {
                    if let Some(p) = built.get(id) {
                        chain.push(p.clone());
                    }
                }
            }
        }
        chain
    }

    pub async fn run_request(
        &self,
        req: &mut Request<axum::body::Body>,
        service_id: Option<Uuid>,
        route_id: Option<Uuid>,
    ) -> Result<(), PluginError> {
        if self.is_empty() {
            return Ok(());
        }
        let chain = self.resolve_chain(service_id, route_id).await;
        for p in &chain {
            p.on_request(req).await?;
        }
        Ok(())
    }

    pub async fn run_response(
        &self,
        resp: &mut Response<BoxBody>,
        service_id: Option<Uuid>,
        route_id: Option<Uuid>,
    ) -> Result<(), PluginError> {
        if self.is_empty() {
            return Ok(());
        }
        let chain = self.resolve_chain(service_id, route_id).await;
        for p in chain.iter().rev() {
            p.on_response(resp).await?;
        }
        Ok(())
    }
}

impl Default for PluginExecutor {
    fn default() -> Self {
        Self::new()
    }
}
