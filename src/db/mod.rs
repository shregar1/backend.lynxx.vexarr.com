pub mod postgres;
pub mod sqlite;

use crate::audit::{AuditEntry, AuditFilter, NewAuditEntry};
use crate::auth::{AdminToken, Role};
use crate::cluster::ClusterBus;
use crate::config::GatewayConfig;
use crate::error::GatewayError;
use crate::models::{Consumer, PluginInstance, Route, Service, Target};
use crate::recording::{RecordingFilter, RecordingIndexRow};
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
    async fn update_target_state(&self, id: &Uuid, state: &str) -> Result<bool, GatewayError>;

    async fn get_service_by_name(&self, name: &str) -> Result<Option<Service>, GatewayError>;

    async fn is_empty(&self) -> Result<bool, GatewayError>;
    async fn seed_from_config(&self, config: &GatewayConfig) -> Result<(), GatewayError>;
    /// Delete all services, routes, and plugins whose `source = 'config'`.
    /// Admin-API-created rows (`source = 'api'`) are untouched.
    async fn clear_config_seed(&self) -> Result<(), GatewayError>;
    /// Mark a set of rows as config-sourced after creation.
    async fn mark_config_source(
        &self,
        service_ids: &[Uuid],
        route_ids: &[Uuid],
        plugin_ids: &[Uuid],
    ) -> Result<(), GatewayError>;

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

    // ---------- quota ----------

    /// Return `(calls_total, calls_rejected)` for `period` ('YYYY-MM'), or
    /// `None` if no row exists yet for that period.
    async fn get_quota_usage(&self, period: &str) -> Result<Option<(u64, u64)>, GatewayError>;

    /// Insert or replace the quota row for `period` with the supplied totals.
    async fn upsert_quota_usage(
        &self,
        period: &str,
        calls_total: u64,
        calls_rejected: u64,
    ) -> Result<(), GatewayError>;

    // ---------- recordings (Time-Travel Debug) ----------

    /// Append a single index row. The corresponding payload should already
    /// have been written to the content-addressed store. Best-effort: the
    /// caller (the request-recorder plugin) logs and drops on failure
    /// rather than failing the request.
    async fn append_recording(&self, row: &RecordingIndexRow) -> Result<(), GatewayError>;

    /// List recordings matching `filter`, newest first, capped by
    /// `filter.effective_limit()`.
    async fn list_recordings(
        &self,
        filter: &RecordingFilter,
    ) -> Result<Vec<RecordingIndexRow>, GatewayError>;

    /// Look up a single recording's index row by sha256 id.
    async fn get_recording_index(
        &self,
        id: &str,
    ) -> Result<Option<RecordingIndexRow>, GatewayError>;

    /// Delete a single recording row. Returns true if a row was removed.
    /// Does NOT touch the on-disk JSON; the admin handler owns that.
    async fn delete_recording(&self, id: &str) -> Result<bool, GatewayError>;

    /// Delete every recording row with `ts < cutoff`. Returns the number
    /// of rows removed. Used by the retention sweeper.
    async fn delete_recordings_before(&self, ts: DateTime<Utc>) -> Result<u64, GatewayError>;

    // ---------- commercial zero-trust ----------

    async fn get_device_fingerprint(
        &self,
        customer_id: i64,
        hardware_id: &str,
    ) -> Result<Option<crate::models::DeviceFingerprint>, GatewayError>;

    async fn create_device_fingerprint(
        &self,
        device: &crate::models::DeviceFingerprint,
    ) -> Result<(), GatewayError>;

    async fn update_device_last_seen(
        &self,
        id: &Uuid,
        ts: DateTime<Utc>,
    ) -> Result<(), GatewayError>;

    async fn create_runtime_grant(
        &self,
        grant: &crate::models::RuntimeGrant,
    ) -> Result<(), GatewayError>;

    async fn get_runtime_grant_by_token_id(
        &self,
        token_id: &str,
    ) -> Result<Option<crate::models::RuntimeGrant>, GatewayError>;

    async fn update_grant_heartbeat(
        &self,
        token_id: &str,
        ts: DateTime<Utc>,
    ) -> Result<(), GatewayError>;

    async fn revoke_grant(&self, token_id: &str) -> Result<bool, GatewayError>;

    async fn get_api_key_by_prefix(
        &self,
        prefix: &str,
    ) -> Result<Option<crate::models::ApiKeyRow>, GatewayError>;
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
