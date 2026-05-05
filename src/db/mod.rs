pub mod postgres;
pub mod sqlite;

use crate::audit::{AuditEntry, AuditFilter, NewAuditEntry};
use crate::auth::{AdminToken, Role};
use crate::cluster::ClusterBus;
use crate::config::GatewayConfig;
use crate::error::GatewayError;
use crate::models::{Consumer, PluginInstance, Route, Service, Target};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use std::sync::Arc;
use uuid::Uuid;

/// Input for `Repository::create_admin_token`. The raw token has already
/// been hashed by the caller — only the hash is persisted.
#[derive(Debug, Clone)]
pub struct NewAdminToken {
    pub id: Uuid,
    pub name: String,
    pub token_hash: String,
    pub role: Role,
    pub created_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
}

/// Patch payload for `Repository::update_admin_token`. None fields are
/// left unchanged.
#[derive(Debug, Clone, Default)]
pub struct AdminTokenPatch {
    pub name: Option<String>,
    pub role: Option<Role>,
    pub expires_at: Option<Option<DateTime<Utc>>>,
}

/// Backend-agnostic persistence layer. Both SQLite and Postgres
/// implementations live behind this trait so the rest of the gateway is
/// storage-agnostic.
#[async_trait]
pub trait Repository: Send + Sync {
    async fn list_services(&self) -> Result<Vec<Service>, GatewayError>;
    async fn get_service(&self, id: &Uuid) -> Result<Option<Service>, GatewayError>;
    async fn create_service(&self, svc: &Service) -> Result<(), GatewayError>;
    async fn update_service(&self, id: &Uuid, svc: &Service) -> Result<(), GatewayError>;
    async fn delete_service(&self, id: &Uuid) -> Result<bool, GatewayError>;

    async fn list_routes(&self) -> Result<Vec<Route>, GatewayError>;
    async fn get_route(&self, id: &Uuid) -> Result<Option<Route>, GatewayError>;
    async fn create_route(&self, route: &Route) -> Result<(), GatewayError>;
    async fn update_route(&self, id: &Uuid, route: &Route) -> Result<(), GatewayError>;
    async fn delete_route(&self, id: &Uuid) -> Result<bool, GatewayError>;

    async fn list_plugins(&self) -> Result<Vec<PluginInstance>, GatewayError>;
    async fn get_plugin(&self, id: &Uuid) -> Result<Option<PluginInstance>, GatewayError>;
    async fn create_plugin(&self, plugin: &PluginInstance) -> Result<(), GatewayError>;
    async fn update_plugin(&self, id: &Uuid, plugin: &PluginInstance) -> Result<(), GatewayError>;
    async fn delete_plugin(&self, id: &Uuid) -> Result<bool, GatewayError>;

    async fn list_consumers(&self) -> Result<Vec<Consumer>, GatewayError>;
    async fn create_consumer(&self, consumer: &Consumer) -> Result<(), GatewayError>;
    async fn delete_consumer(&self, id: &Uuid) -> Result<bool, GatewayError>;

    async fn list_targets(&self) -> Result<Vec<Target>, GatewayError>;
    async fn list_targets_for_service(
        &self,
        service_id: &Uuid,
    ) -> Result<Vec<Target>, GatewayError>;
    async fn get_target(&self, id: &Uuid) -> Result<Option<Target>, GatewayError>;
    async fn create_target(&self, target: &Target) -> Result<(), GatewayError>;
    async fn update_target(&self, id: &Uuid, target: &Target) -> Result<(), GatewayError>;
    async fn delete_target(&self, id: &Uuid) -> Result<bool, GatewayError>;

    async fn is_empty(&self) -> Result<bool, GatewayError>;
    async fn seed_from_config(&self, config: &GatewayConfig) -> Result<(), GatewayError>;

    // ---------- admin tokens ----------

    async fn list_admin_tokens(&self) -> Result<Vec<AdminToken>, GatewayError>;
    async fn get_admin_token_by_hash(
        &self,
        token_hash: &str,
    ) -> Result<Option<AdminToken>, GatewayError>;
    async fn get_admin_token(&self, id: &Uuid) -> Result<Option<AdminToken>, GatewayError>;
    async fn create_admin_token(&self, token: &NewAdminToken) -> Result<(), GatewayError>;
    async fn update_admin_token(
        &self,
        id: &Uuid,
        patch: &AdminTokenPatch,
    ) -> Result<Option<AdminToken>, GatewayError>;
    async fn delete_admin_token(&self, id: &Uuid) -> Result<bool, GatewayError>;
    async fn update_admin_token_last_used(
        &self,
        id: &Uuid,
        ts: DateTime<Utc>,
    ) -> Result<(), GatewayError>;
    async fn has_any_admin_token(&self) -> Result<bool, GatewayError>;

    // ---------- audit log ----------

    async fn append_audit(&self, entry: &NewAuditEntry) -> Result<AuditEntry, GatewayError>;
    async fn list_audit(&self, filter: &AuditFilter) -> Result<Vec<AuditEntry>, GatewayError>;
    async fn last_audit_hash(&self) -> Result<Option<String>, GatewayError>;

    /// Return a cluster bus for fan-out of admin mutations across nodes.
    /// Postgres uses LISTEN/NOTIFY; SQLite polls a `_sync_log` table.
    fn cluster_bus(&self) -> Arc<dyn ClusterBus>;
}

/// Build the appropriate repository implementation from the configured URL.
/// `postgres://` and `postgresql://` schemes route to Postgres; everything
/// else (typically `sqlite://...`) routes to SQLite.
pub async fn connect(url: &str) -> Result<Arc<dyn Repository>, GatewayError> {
    if url.starts_with("postgres://") || url.starts_with("postgresql://") {
        let repo = postgres::PostgresRepository::new(url).await?;
        Ok(Arc::new(repo))
    } else {
        let repo = sqlite::SqliteRepository::new(url).await?;
        Ok(Arc::new(repo))
    }
}
