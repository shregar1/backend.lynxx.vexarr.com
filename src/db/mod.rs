pub mod postgres;
pub mod sqlite;

use crate::cluster::ClusterBus;
use crate::config::GatewayConfig;
use crate::error::GatewayError;
use crate::models::{Consumer, PluginInstance, Route, Service, Target};
use async_trait::async_trait;
use std::sync::Arc;
use uuid::Uuid;

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
