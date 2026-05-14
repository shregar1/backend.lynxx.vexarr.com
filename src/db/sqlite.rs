use crate::audit::{compute_hash, AuditEntry, AuditFilter, NewAuditEntry};
use crate::auth::{AdminToken, Role};
use crate::cluster::sqlite::SqliteBus;
use crate::cluster::ClusterBus;
use crate::config::GatewayConfig;
use crate::db::{AdminTokenPatch, NewAdminToken, Repository};
use crate::error::GatewayError;
use crate::db::LogFilter;
use crate::models::{
    AlertEvent, AlertRule, Consumer, ConsumerGroup, ConsumerGroupMember, ErrorPage,
    GeoBucket, IpRule, LogEntry, MetricBucket, PluginInstance, Probe, ProbeResult,
    Project, RateLimitBucket, Route, Service, Snapshot, Target,
};
use crate::recording::{RecordingFilter, RecordingIndexRow};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::sqlite::{SqlitePool, SqlitePoolOptions};
use sqlx::Row;
use std::str::FromStr;
use std::sync::Arc;
use uuid::Uuid;

/// SQLite-backed repository. Source of truth for services, routes, plugins,
/// and consumers when the gateway runs in single-node / dev mode.
pub struct SqliteRepository {
    pool: SqlitePool,
}

impl SqliteRepository {
    pub async fn new(database_url: &str) -> Result<Self, GatewayError> {
        let pool = SqlitePoolOptions::new()
            .max_connections(8)
            .after_connect(|conn, _meta| {
                Box::pin(async move {
                    sqlx::query("PRAGMA foreign_keys = ON")
                        .execute(&mut *conn)
                        .await?;
                    Ok(())
                })
            })
            .connect(database_url)
            .await?;

        sqlx::migrate!("./migrations/sqlite").run(&pool).await?;

        Ok(Self { pool })
    }

    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }
}

#[async_trait]
impl Repository for SqliteRepository {
    // ---------- projects ----------

    async fn list_projects(&self) -> Result<Vec<Project>, GatewayError> {
        let rows = sqlx::query(
            "SELECT id, name, description, created_at, updated_at \
             FROM projects WHERE is_deleted = 0",
        )
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(row_to_project).collect()
    }

    async fn get_project(&self, id: &Uuid) -> Result<Option<Project>, GatewayError> {
        let row = sqlx::query(
            "SELECT id, name, description, created_at, updated_at \
             FROM projects WHERE id = ? AND is_deleted = 0",
        )
        .bind(id.to_string())
        .fetch_optional(&self.pool)
        .await?;
        row.map(row_to_project).transpose()
    }

    async fn create_project(&self, project: &Project) -> Result<(), GatewayError> {
        sqlx::query(
            "INSERT INTO projects (id, name, description, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(project.id.to_string())
        .bind(&project.name)
        .bind(&project.description)
        .bind(project.created_at.to_rfc3339())
        .bind(project.updated_at.to_rfc3339())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn update_project(&self, id: &Uuid, project: &Project) -> Result<(), GatewayError> {
        sqlx::query(
            "UPDATE projects SET name = ?, description = ?, updated_at = ? \
             WHERE id = ? AND is_deleted = 0",
        )
        .bind(&project.name)
        .bind(&project.description)
        .bind(project.updated_at.to_rfc3339())
        .bind(id.to_string())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn delete_project(&self, id: &Uuid) -> Result<bool, GatewayError> {
        let result = sqlx::query(
            "UPDATE projects SET is_deleted = 1, name = name || '__deleted_' || id \
             WHERE id = ? AND is_deleted = 0",
        )
        .bind(id.to_string())
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() > 0)
    }

    async fn list_services_for_project(
        &self,
        project_id: &Uuid,
    ) -> Result<Vec<Service>, GatewayError> {
        let rows = sqlx::query(
            "SELECT id, name, url, retries, connect_timeout_ms, lb_strategy, service_type, \
                    document_root, index_file, spa_fallback, \
                    tls_verify, tls_client_cert_path, tls_client_key_path, tls_ca_path, \
                    project_id, domain, subdomain, \
                    created_at, updated_at \
             FROM services WHERE project_id = ? AND is_deleted = 0",
        )
        .bind(project_id.to_string())
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(row_to_service).collect()
    }

    // ---------- services ----------

    async fn list_services(&self) -> Result<Vec<Service>, GatewayError> {
        let rows = sqlx::query(
            "SELECT id, name, url, retries, connect_timeout_ms, lb_strategy, service_type, \
                    document_root, index_file, spa_fallback, \
                    tls_verify, tls_client_cert_path, tls_client_key_path, tls_ca_path, \
                    project_id, domain, subdomain, \
                    created_at, updated_at \
             FROM services WHERE is_deleted = 0",
        )
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter().map(row_to_service).collect()
    }

    async fn get_service(&self, id: &Uuid) -> Result<Option<Service>, GatewayError> {
        let row = sqlx::query(
            "SELECT id, name, url, retries, connect_timeout_ms, lb_strategy, service_type, \
                    document_root, index_file, spa_fallback, \
                    tls_verify, tls_client_cert_path, tls_client_key_path, tls_ca_path, \
                    project_id, domain, subdomain, \
                    created_at, updated_at \
             FROM services WHERE id = ? AND is_deleted = 0",
        )
        .bind(id.to_string())
        .fetch_optional(&self.pool)
        .await?;

        row.map(row_to_service).transpose()
    }

    async fn get_service_by_name(&self, name: &str) -> Result<Option<Service>, GatewayError> {
        let row = sqlx::query(
            "SELECT id, name, url, retries, connect_timeout_ms, lb_strategy, service_type, \
                    document_root, index_file, spa_fallback, \
                    tls_verify, tls_client_cert_path, tls_client_key_path, tls_ca_path, \
                    project_id, domain, subdomain, \
                    created_at, updated_at \
             FROM services WHERE name = ? AND is_deleted = 0",
        )
        .bind(name)
        .fetch_optional(&self.pool)
        .await?;

        row.map(row_to_service).transpose()
    }

    async fn create_service(&self, svc: &Service) -> Result<(), GatewayError> {
        sqlx::query(
            "INSERT INTO services (id, name, url, retries, connect_timeout_ms, lb_strategy, service_type, \
                                   document_root, index_file, spa_fallback, \
                                   tls_verify, tls_client_cert_path, tls_client_key_path, tls_ca_path, \
                                   project_id, domain, subdomain, \
                                   enabled, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 1, ?, ?)",
        )
        .bind(svc.id.to_string())
        .bind(&svc.name)
        .bind(&svc.url)
        .bind(svc.retries as i64)
        .bind(svc.connect_timeout_ms as i64)
        .bind(&svc.lb_strategy)
        .bind(&svc.service_type)
        .bind(svc.document_root.as_deref())
        .bind(&svc.index_file)
        .bind(svc.spa_fallback as i64)
        .bind(svc.tls_verify as i64)
        .bind(svc.tls_client_cert_path.as_deref())
        .bind(svc.tls_client_key_path.as_deref())
        .bind(svc.tls_ca_path.as_deref())
        .bind(svc.project_id.map(|id| id.to_string()))
        .bind(&svc.domain)
        .bind(&svc.subdomain)
        .bind(svc.created_at.to_rfc3339())
        .bind(svc.updated_at.to_rfc3339())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn update_service(&self, id: &Uuid, svc: &Service) -> Result<(), GatewayError> {
        sqlx::query(
            "UPDATE services SET name = ?, url = ?, retries = ?, connect_timeout_ms = ?, lb_strategy = ?, service_type = ?, \
                                 document_root = ?, index_file = ?, spa_fallback = ?, \
                                 tls_verify = ?, tls_client_cert_path = ?, tls_client_key_path = ?, tls_ca_path = ?, \
                                 project_id = ?, domain = ?, subdomain = ?, \
                                 updated_at = ? \
             WHERE id = ? AND is_deleted = 0",
        )
        .bind(&svc.name)
        .bind(&svc.url)
        .bind(svc.retries as i64)
        .bind(svc.connect_timeout_ms as i64)
        .bind(&svc.lb_strategy)
        .bind(&svc.service_type)
        .bind(svc.document_root.as_deref())
        .bind(&svc.index_file)
        .bind(svc.spa_fallback as i64)
        .bind(svc.tls_verify as i64)
        .bind(svc.tls_client_cert_path.as_deref())
        .bind(svc.tls_client_key_path.as_deref())
        .bind(svc.tls_ca_path.as_deref())
        .bind(svc.project_id.map(|id| id.to_string()))
        .bind(&svc.domain)
        .bind(&svc.subdomain)
        .bind(svc.updated_at.to_rfc3339())
        .bind(id.to_string())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn delete_service(&self, id: &Uuid) -> Result<bool, GatewayError> {
        let result = sqlx::query(
            "UPDATE services SET is_deleted = 1, name = name || '__deleted_' || id \
             WHERE id = ? AND is_deleted = 0",
        )
        .bind(id.to_string())
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() > 0)
    }

    // ---------- targets ----------

    async fn list_targets(&self) -> Result<Vec<Target>, GatewayError> {
        let rows = sqlx::query(
            "SELECT id, service_id, url, weight, enabled, zone, state, created_at, updated_at FROM targets WHERE is_deleted = 0",
        )
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(row_to_target).collect()
    }

    async fn list_targets_for_service(
        &self,
        service_id: &Uuid,
    ) -> Result<Vec<Target>, GatewayError> {
        let rows = sqlx::query(
            "SELECT id, service_id, url, weight, enabled, zone, state, created_at, updated_at \
             FROM targets WHERE service_id = ? AND is_deleted = 0",
        )
        .bind(service_id.to_string())
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(row_to_target).collect()
    }

    async fn get_target(&self, id: &Uuid) -> Result<Option<Target>, GatewayError> {
        let row = sqlx::query(
            "SELECT id, service_id, url, weight, enabled, zone, state, created_at, updated_at \
             FROM targets WHERE id = ? AND is_deleted = 0",
        )
        .bind(id.to_string())
        .fetch_optional(&self.pool)
        .await?;
        row.map(row_to_target).transpose()
    }

    async fn create_target(&self, target: &Target) -> Result<(), GatewayError> {
        sqlx::query(
            "INSERT INTO targets (id, service_id, url, weight, enabled, zone, state, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(target.id.to_string())
        .bind(target.service_id.to_string())
        .bind(&target.url)
        .bind(target.weight as i64)
        .bind(target.enabled as i64)
        .bind(target.zone.as_deref())
        .bind(target.state.as_str())
        .bind(target.created_at.to_rfc3339())
        .bind(target.updated_at.to_rfc3339())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn update_target(&self, id: &Uuid, target: &Target) -> Result<(), GatewayError> {
        sqlx::query(
            "UPDATE targets SET url = ?, weight = ?, enabled = ?, zone = ?, state = ?, updated_at = ? WHERE id = ? AND is_deleted = 0",
        )
        .bind(&target.url)
        .bind(target.weight as i64)
        .bind(target.enabled as i64)
        .bind(target.zone.as_deref())
        .bind(target.state.as_str())
        .bind(target.updated_at.to_rfc3339())
        .bind(id.to_string())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn delete_target(&self, id: &Uuid) -> Result<bool, GatewayError> {
        let result = sqlx::query("UPDATE targets SET is_deleted = 1 WHERE id = ? AND is_deleted = 0")
            .bind(id.to_string())
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    async fn update_target_state(&self, id: &Uuid, state: &str) -> Result<bool, GatewayError> {
        let result = sqlx::query(
            "UPDATE targets SET state = ?, updated_at = ? WHERE id = ? AND is_deleted = 0",
        )
        .bind(state)
        .bind(Utc::now().to_rfc3339())
        .bind(id.to_string())
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() > 0)
    }

    // ---------- routes ----------

    async fn list_routes(&self) -> Result<Vec<Route>, GatewayError> {
        let rows = sqlx::query(
            "SELECT id, service_id, paths, methods, hosts, strip_path, created_at, updated_at \
             FROM routes WHERE is_deleted = 0",
        )
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter().map(row_to_route).collect()
    }

    async fn get_route(&self, id: &Uuid) -> Result<Option<Route>, GatewayError> {
        let row = sqlx::query(
            "SELECT id, service_id, paths, methods, hosts, strip_path, created_at, updated_at \
             FROM routes WHERE id = ? AND is_deleted = 0",
        )
        .bind(id.to_string())
        .fetch_optional(&self.pool)
        .await?;

        row.map(row_to_route).transpose()
    }

    async fn create_route(&self, route: &Route) -> Result<(), GatewayError> {
        let paths = serde_json::to_string(&route.paths)
            .map_err(|e| GatewayError::Internal(e.to_string()))?;
        let methods = serde_json::to_string(&route.methods)
            .map_err(|e| GatewayError::Internal(e.to_string()))?;
        let hosts = serde_json::to_string(&route.hosts)
            .map_err(|e| GatewayError::Internal(e.to_string()))?;

        sqlx::query(
            "INSERT INTO routes (id, service_id, paths, methods, hosts, strip_path, enabled, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, 1, ?, ?)",
        )
        .bind(route.id.to_string())
        .bind(route.service_id.to_string())
        .bind(paths)
        .bind(methods)
        .bind(hosts)
        .bind(route.strip_path as i64)
        .bind(route.created_at.to_rfc3339())
        .bind(route.updated_at.to_rfc3339())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn update_route(&self, id: &Uuid, route: &Route) -> Result<(), GatewayError> {
        let paths = serde_json::to_string(&route.paths)
            .map_err(|e| GatewayError::Internal(e.to_string()))?;
        let methods = serde_json::to_string(&route.methods)
            .map_err(|e| GatewayError::Internal(e.to_string()))?;
        let hosts = serde_json::to_string(&route.hosts)
            .map_err(|e| GatewayError::Internal(e.to_string()))?;

        sqlx::query(
            "UPDATE routes SET service_id = ?, paths = ?, methods = ?, hosts = ?, strip_path = ?, updated_at = ? \
             WHERE id = ? AND is_deleted = 0",
        )
        .bind(route.service_id.to_string())
        .bind(paths)
        .bind(methods)
        .bind(hosts)
        .bind(route.strip_path as i64)
        .bind(route.updated_at.to_rfc3339())
        .bind(id.to_string())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn delete_route(&self, id: &Uuid) -> Result<bool, GatewayError> {
        let result = sqlx::query("UPDATE routes SET is_deleted = 1 WHERE id = ? AND is_deleted = 0")
            .bind(id.to_string())
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    // ---------- plugins ----------

    async fn list_plugins(&self) -> Result<Vec<PluginInstance>, GatewayError> {
        let rows = sqlx::query(
            "SELECT id, name, service_id, route_id, enabled, config, created_at, updated_at FROM plugins WHERE is_deleted = 0",
        )
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter().map(row_to_plugin).collect()
    }

    async fn get_plugin(&self, id: &Uuid) -> Result<Option<PluginInstance>, GatewayError> {
        let row = sqlx::query(
            "SELECT id, name, service_id, route_id, enabled, config, created_at, updated_at \
             FROM plugins WHERE id = ? AND is_deleted = 0",
        )
        .bind(id.to_string())
        .fetch_optional(&self.pool)
        .await?;
        row.map(row_to_plugin).transpose()
    }

    async fn create_plugin(&self, plugin: &PluginInstance) -> Result<(), GatewayError> {
        let config = serde_json::to_string(&plugin.config)
            .map_err(|e| GatewayError::Internal(e.to_string()))?;

        sqlx::query(
            "INSERT INTO plugins (id, name, service_id, route_id, enabled, config, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(plugin.id.to_string())
        .bind(&plugin.name)
        .bind(plugin.service_id.map(|v| v.to_string()))
        .bind(plugin.route_id.map(|v| v.to_string()))
        .bind(plugin.enabled as i64)
        .bind(config)
        .bind(plugin.created_at.to_rfc3339())
        .bind(plugin.updated_at.to_rfc3339())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn update_plugin(&self, id: &Uuid, plugin: &PluginInstance) -> Result<(), GatewayError> {
        let config = serde_json::to_string(&plugin.config)
            .map_err(|e| GatewayError::Internal(e.to_string()))?;
        sqlx::query(
            "UPDATE plugins SET name = ?, service_id = ?, route_id = ?, enabled = ?, config = ?, updated_at = ? \
             WHERE id = ? AND is_deleted = 0",
        )
        .bind(&plugin.name)
        .bind(plugin.service_id.map(|v| v.to_string()))
        .bind(plugin.route_id.map(|v| v.to_string()))
        .bind(plugin.enabled as i64)
        .bind(config)
        .bind(plugin.updated_at.to_rfc3339())
        .bind(id.to_string())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn delete_plugin(&self, id: &Uuid) -> Result<bool, GatewayError> {
        let result = sqlx::query("UPDATE plugins SET is_deleted = 1 WHERE id = ? AND is_deleted = 0")
            .bind(id.to_string())
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    // ---------- consumers ----------

    async fn list_consumers(&self) -> Result<Vec<Consumer>, GatewayError> {
        let rows = sqlx::query(
            "SELECT id, username, custom_id, api_key, created_at, updated_at FROM consumers WHERE is_deleted = 0",
        )
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter().map(row_to_consumer).collect()
    }

    async fn create_consumer(&self, consumer: &Consumer) -> Result<(), GatewayError> {
        sqlx::query(
            "INSERT INTO consumers (id, username, custom_id, api_key, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(consumer.id.to_string())
        .bind(&consumer.username)
        .bind(consumer.custom_id.as_deref())
        .bind(consumer.api_key.as_deref())
        .bind(consumer.created_at.to_rfc3339())
        .bind(consumer.updated_at.to_rfc3339())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn delete_consumer(&self, id: &Uuid) -> Result<bool, GatewayError> {
        let result = sqlx::query(
            "UPDATE consumers SET is_deleted = 1, username = username || '__deleted_' || id \
             WHERE id = ? AND is_deleted = 0",
        )
        .bind(id.to_string())
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() > 0)
    }

    // ---------- cascade soft-delete ----------

    async fn soft_delete_plugins_for_route(
        &self,
        route_id: &Uuid,
    ) -> Result<u64, GatewayError> {
        let result = sqlx::query(
            "UPDATE plugins SET is_deleted = 1 WHERE route_id = ? AND is_deleted = 0",
        )
        .bind(route_id.to_string())
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected())
    }

    async fn soft_delete_plugins_for_service(
        &self,
        service_id: &Uuid,
    ) -> Result<u64, GatewayError> {
        let result = sqlx::query(
            "UPDATE plugins SET is_deleted = 1 WHERE service_id = ? AND is_deleted = 0",
        )
        .bind(service_id.to_string())
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected())
    }

    async fn soft_delete_routes_for_service(
        &self,
        service_id: &Uuid,
    ) -> Result<Vec<Uuid>, GatewayError> {
        let rows: Vec<(String,)> = sqlx::query_as(
            "SELECT id FROM routes WHERE service_id = ? AND is_deleted = 0",
        )
        .bind(service_id.to_string())
        .fetch_all(&self.pool)
        .await?;

        let ids: Vec<Uuid> = rows
            .iter()
            .filter_map(|(id,)| Uuid::from_str(id).ok())
            .collect();

        sqlx::query(
            "UPDATE routes SET is_deleted = 1 WHERE service_id = ? AND is_deleted = 0",
        )
        .bind(service_id.to_string())
        .execute(&self.pool)
        .await?;

        Ok(ids)
    }

    async fn soft_delete_targets_for_service(
        &self,
        service_id: &Uuid,
    ) -> Result<u64, GatewayError> {
        let result = sqlx::query(
            "UPDATE targets SET is_deleted = 1 WHERE service_id = ? AND is_deleted = 0",
        )
        .bind(service_id.to_string())
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected())
    }

    async fn soft_delete_services_for_project(
        &self,
        project_id: &Uuid,
    ) -> Result<Vec<Uuid>, GatewayError> {
        let rows: Vec<(String,)> = sqlx::query_as(
            "SELECT id FROM services WHERE project_id = ? AND is_deleted = 0",
        )
        .bind(project_id.to_string())
        .fetch_all(&self.pool)
        .await?;

        let ids: Vec<Uuid> = rows
            .iter()
            .filter_map(|(id,)| Uuid::from_str(id).ok())
            .collect();

        sqlx::query(
            "UPDATE services SET is_deleted = 1, name = name || '__deleted_' || id \
             WHERE project_id = ? AND is_deleted = 0",
        )
        .bind(project_id.to_string())
        .execute(&self.pool)
        .await?;

        Ok(ids)
    }

    // ---------- bootstrap helpers ----------

    async fn is_empty(&self) -> Result<bool, GatewayError> {
        let total: i64 = sqlx::query_scalar(
            "SELECT (SELECT COUNT(*) FROM services WHERE is_deleted = 0) \
                  + (SELECT COUNT(*) FROM routes WHERE is_deleted = 0) \
                  + (SELECT COUNT(*) FROM plugins WHERE is_deleted = 0) \
                  + (SELECT COUNT(*) FROM consumers WHERE is_deleted = 0)",
        )
        .fetch_one(&self.pool)
        .await?;
        Ok(total == 0)
    }

    async fn seed_from_config(&self, config: &GatewayConfig) -> Result<(), GatewayError> {
        seed_from_config_generic(self, config).await
    }

    async fn clear_config_seed(&self) -> Result<(), GatewayError> {
        sqlx::query("DELETE FROM plugins WHERE source = 'config'")
            .execute(&self.pool)
            .await?;
        sqlx::query("DELETE FROM routes WHERE source = 'config'")
            .execute(&self.pool)
            .await?;
        sqlx::query("DELETE FROM services WHERE source = 'config'")
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn mark_config_source(
        &self,
        service_ids: &[Uuid],
        route_ids: &[Uuid],
        plugin_ids: &[Uuid],
    ) -> Result<(), GatewayError> {
        for id in service_ids {
            sqlx::query("UPDATE services SET source = 'config' WHERE id = ?")
                .bind(id.to_string())
                .execute(&self.pool)
                .await?;
        }
        for id in route_ids {
            sqlx::query("UPDATE routes SET source = 'config' WHERE id = ?")
                .bind(id.to_string())
                .execute(&self.pool)
                .await?;
        }
        for id in plugin_ids {
            sqlx::query("UPDATE plugins SET source = 'config' WHERE id = ?")
                .bind(id.to_string())
                .execute(&self.pool)
                .await?;
        }
        Ok(())
    }

    fn cluster_bus(&self) -> Arc<dyn ClusterBus> {
        Arc::new(SqliteBus::new(self.pool.clone()))
    }

    // ---------- admin tokens ----------

    async fn list_admin_tokens(&self) -> Result<Vec<AdminToken>, GatewayError> {
        let rows = sqlx::query(
            "SELECT id, name, role, created_at, expires_at, last_used_at \
             FROM admin_tokens WHERE is_deleted = 0 ORDER BY created_at ASC",
        )
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(row_to_admin_token).collect()
    }

    async fn get_admin_token_by_hash(
        &self,
        token_hash: &str,
    ) -> Result<Option<AdminToken>, GatewayError> {
        let row = sqlx::query(
            "SELECT id, name, role, created_at, expires_at, last_used_at \
             FROM admin_tokens WHERE token_hash = ? AND is_deleted = 0",
        )
        .bind(token_hash)
        .fetch_optional(&self.pool)
        .await?;
        row.map(row_to_admin_token).transpose()
    }

    async fn get_admin_token(&self, id: &Uuid) -> Result<Option<AdminToken>, GatewayError> {
        let row = sqlx::query(
            "SELECT id, name, role, created_at, expires_at, last_used_at \
             FROM admin_tokens WHERE id = ? AND is_deleted = 0",
        )
        .bind(id.to_string())
        .fetch_optional(&self.pool)
        .await?;
        row.map(row_to_admin_token).transpose()
    }

    async fn create_admin_token(&self, token: &NewAdminToken) -> Result<(), GatewayError> {
        sqlx::query(
            "INSERT INTO admin_tokens (id, name, token_hash, role, created_at, expires_at) \
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(token.id.to_string())
        .bind(&token.name)
        .bind(&token.token_hash)
        .bind(token.role.to_string())
        .bind(token.created_at.to_rfc3339())
        .bind(token.expires_at.map(|d| d.to_rfc3339()))
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn update_admin_token(
        &self,
        id: &Uuid,
        patch: &AdminTokenPatch,
    ) -> Result<Option<AdminToken>, GatewayError> {
        // Read-modify-write under a single transaction; the table is tiny and
        // updates are rare so the simpler shape beats a hand-built UPDATE.
        let mut tx = self.pool.begin().await?;
        let row = sqlx::query(
            "SELECT id, name, role, created_at, expires_at, last_used_at \
             FROM admin_tokens WHERE id = ? AND is_deleted = 0",
        )
        .bind(id.to_string())
        .fetch_optional(&mut *tx)
        .await?;
        let Some(row) = row else {
            return Ok(None);
        };
        let mut existing = row_to_admin_token(row)?;
        if let Some(name) = patch.name.clone() {
            existing.name = name;
        }
        if let Some(role) = patch.role {
            existing.role = role;
        }
        if let Some(exp) = patch.expires_at {
            existing.expires_at = exp;
        }
        sqlx::query("UPDATE admin_tokens SET name = ?, role = ?, expires_at = ? WHERE id = ?")
            .bind(&existing.name)
            .bind(existing.role.to_string())
            .bind(existing.expires_at.map(|d| d.to_rfc3339()))
            .bind(id.to_string())
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(Some(existing))
    }

    async fn delete_admin_token(&self, id: &Uuid) -> Result<bool, GatewayError> {
        let result = sqlx::query(
            "UPDATE admin_tokens SET is_deleted = 1, token_hash = token_hash || '__deleted_' || id \
             WHERE id = ? AND is_deleted = 0",
        )
        .bind(id.to_string())
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() > 0)
    }

    async fn update_admin_token_last_used(
        &self,
        id: &Uuid,
        ts: DateTime<Utc>,
    ) -> Result<(), GatewayError> {
        sqlx::query("UPDATE admin_tokens SET last_used_at = ? WHERE id = ?")
            .bind(ts.to_rfc3339())
            .bind(id.to_string())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn has_any_admin_token(&self) -> Result<bool, GatewayError> {
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM admin_tokens WHERE is_deleted = 0")
            .fetch_one(&self.pool)
            .await?;
        Ok(count > 0)
    }

    // ---------- audit log ----------

    async fn append_audit(&self, entry: &NewAuditEntry) -> Result<AuditEntry, GatewayError> {
        // Hash chain: read the latest hash, compute new hash, insert.
        // We do this in a transaction so concurrent admin writers serialise
        // through here without two appends sharing the same prev_hash.
        let mut tx = self.pool.begin().await?;
        let prev_hash: Option<String> =
            sqlx::query_scalar("SELECT hash FROM audit_log ORDER BY id DESC LIMIT 1")
                .fetch_optional(&mut *tx)
                .await?;
        let hash = compute_hash(prev_hash.as_deref(), entry);
        let diff_str = entry
            .diff
            .as_ref()
            .map(|v| serde_json::to_string(v))
            .transpose()
            .map_err(|e| GatewayError::Internal(format!("audit diff serialize: {e}")))?;

        let result = sqlx::query(
            "INSERT INTO audit_log (ts, actor_id, actor_name, actor_role, action, resource_type, resource_id, diff, hash, prev_hash) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(entry.ts.to_rfc3339())
        .bind(entry.actor_id.map(|u| u.to_string()))
        .bind(entry.actor_name.as_deref())
        .bind(entry.actor_role.as_deref())
        .bind(&entry.action)
        .bind(&entry.resource_type)
        .bind(entry.resource_id.as_deref())
        .bind(diff_str)
        .bind(&hash)
        .bind(prev_hash.as_deref())
        .execute(&mut *tx)
        .await?;
        let id = result.last_insert_rowid();
        tx.commit().await?;

        Ok(AuditEntry {
            id,
            ts: entry.ts,
            actor_id: entry.actor_id,
            actor_name: entry.actor_name.clone(),
            actor_role: entry.actor_role.clone(),
            action: entry.action.clone(),
            resource_type: entry.resource_type.clone(),
            resource_id: entry.resource_id.clone(),
            diff: entry.diff.clone(),
            hash,
            prev_hash,
        })
    }

    async fn list_audit(&self, filter: &AuditFilter) -> Result<Vec<AuditEntry>, GatewayError> {
        let limit = filter.effective_limit() as i64;
        // QueryBuilder would be nice but we avoid it for parity with the
        // rest of this file; manual params it is.
        let diff_col = if filter.include_diff { "diff" } else { "NULL AS diff" };
        let mut sql = format!(
            "SELECT id, ts, actor_id, actor_name, actor_role, action, resource_type, resource_id, {diff_col}, hash, prev_hash FROM audit_log WHERE 1=1",
        );
        if filter.actor.is_some() {
            sql.push_str(" AND (actor_id = ? OR actor_name = ?)");
        }
        if filter.action.is_some() {
            sql.push_str(" AND action = ?");
        }
        if filter.resource_type.is_some() {
            sql.push_str(" AND resource_type = ?");
        }
        if filter.since.is_some() {
            sql.push_str(" AND ts >= ?");
        }
        sql.push_str(" ORDER BY ts DESC, id DESC LIMIT ?");

        let mut q = sqlx::query(&sql);
        if let Some(actor) = &filter.actor {
            q = q.bind(actor).bind(actor);
        }
        if let Some(action) = &filter.action {
            q = q.bind(action);
        }
        if let Some(rt) = &filter.resource_type {
            q = q.bind(rt);
        }
        if let Some(since) = filter.since {
            q = q.bind(since.to_rfc3339());
        }
        q = q.bind(limit);

        let rows = q.fetch_all(&self.pool).await?;
        rows.into_iter().map(row_to_audit).collect()
    }

    async fn last_audit_hash(&self) -> Result<Option<String>, GatewayError> {
        let v: Option<String> =
            sqlx::query_scalar("SELECT hash FROM audit_log ORDER BY id DESC LIMIT 1")
                .fetch_optional(&self.pool)
                .await?;
        Ok(v)
    }

    // ---------- recordings ----------

    async fn append_recording(&self, row: &RecordingIndexRow) -> Result<(), GatewayError> {
        sqlx::query(
            "INSERT INTO recordings (id, ts, method, path, status, duration_ms, service_id, route_id, consumer, bytes_request, bytes_response) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&row.id)
        .bind(row.ts.to_rfc3339())
        .bind(&row.method)
        .bind(&row.path)
        .bind(row.status as i64)
        .bind(row.duration_ms as i64)
        .bind(row.service_id.map(|u| u.to_string()))
        .bind(row.route_id.map(|u| u.to_string()))
        .bind(row.consumer.as_deref())
        .bind(row.bytes_request as i64)
        .bind(row.bytes_response as i64)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn list_recordings(
        &self,
        filter: &RecordingFilter,
    ) -> Result<Vec<RecordingIndexRow>, GatewayError> {
        let limit = filter.effective_limit() as i64;
        let mut sql = String::from(
            "SELECT id, ts, method, path, status, duration_ms, service_id, route_id, consumer, bytes_request, bytes_response \
             FROM recordings WHERE is_deleted = 0",
        );
        if filter.since.is_some() {
            sql.push_str(" AND ts >= ?");
        }
        if filter.method.is_some() {
            sql.push_str(" AND method = ?");
        }
        if filter.path_prefix.is_some() {
            sql.push_str(" AND path LIKE ?");
        }
        if filter.status.is_some() {
            sql.push_str(" AND status = ?");
        }
        if filter.service_id.is_some() {
            sql.push_str(" AND service_id = ?");
        }
        sql.push_str(" ORDER BY ts DESC LIMIT ?");

        let mut q = sqlx::query(&sql);
        if let Some(since) = filter.since {
            q = q.bind(since.to_rfc3339());
        }
        if let Some(m) = &filter.method {
            q = q.bind(m);
        }
        let path_like;
        if let Some(p) = &filter.path_prefix {
            path_like = format!("{}%", p);
            q = q.bind(path_like);
        }
        if let Some(st) = filter.status {
            q = q.bind(st as i64);
        }
        if let Some(sid) = filter.service_id {
            q = q.bind(sid.to_string());
        }
        q = q.bind(limit);

        let rows = q.fetch_all(&self.pool).await?;
        rows.into_iter().map(row_to_recording_index).collect()
    }

    async fn get_recording_index(
        &self,
        id: &str,
    ) -> Result<Option<RecordingIndexRow>, GatewayError> {
        let row = sqlx::query(
            "SELECT id, ts, method, path, status, duration_ms, service_id, route_id, consumer, bytes_request, bytes_response \
             FROM recordings WHERE id = ? AND is_deleted = 0",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        row.map(row_to_recording_index).transpose()
    }

    async fn delete_recording(&self, id: &str) -> Result<bool, GatewayError> {
        let result = sqlx::query("UPDATE recordings SET is_deleted = 1 WHERE id = ? AND is_deleted = 0")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    async fn delete_recordings_before(&self, ts: DateTime<Utc>) -> Result<u64, GatewayError> {
        let result = sqlx::query("UPDATE recordings SET is_deleted = 1 WHERE ts < ? AND is_deleted = 0")
            .bind(ts.to_rfc3339())
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected())
    }

    // ---------- commercial zero-trust (Not implemented for SQLite) ----------

    async fn get_device_fingerprint(
        &self,
        _customer_id: i64,
        _hardware_id: &str,
    ) -> Result<Option<crate::models::DeviceFingerprint>, GatewayError> {
        Err(GatewayError::Internal("Not implemented for SQLite".into()))
    }

    async fn create_device_fingerprint(
        &self,
        _device: &crate::models::DeviceFingerprint,
    ) -> Result<(), GatewayError> {
        Err(GatewayError::Internal("Not implemented for SQLite".into()))
    }

    async fn update_device_last_seen(
        &self,
        _id: &Uuid,
        _ts: DateTime<Utc>,
    ) -> Result<(), GatewayError> {
        Err(GatewayError::Internal("Not implemented for SQLite".into()))
    }

    async fn create_runtime_grant(
        &self,
        _grant: &crate::models::RuntimeGrant,
    ) -> Result<(), GatewayError> {
        Err(GatewayError::Internal("Not implemented for SQLite".into()))
    }

    async fn get_runtime_grant_by_token_id(
        &self,
        _token_id: &str,
    ) -> Result<Option<crate::models::RuntimeGrant>, GatewayError> {
        Err(GatewayError::Internal("Not implemented for SQLite".into()))
    }

    async fn update_grant_heartbeat(
        &self,
        _token_id: &str,
        _ts: DateTime<Utc>,
    ) -> Result<(), GatewayError> {
        Err(GatewayError::Internal("Not implemented for SQLite".into()))
    }

    async fn revoke_grant(&self, _token_id: &str) -> Result<bool, GatewayError> {
        Err(GatewayError::Internal("Not implemented for SQLite".into()))
    }

    async fn get_api_key_by_prefix(
        &self,
        _prefix: &str,
    ) -> Result<Option<crate::models::ApiKeyRow>, GatewayError> {
        Err(GatewayError::Internal("Not implemented for SQLite".into()))
    }

    // ---------- quota ----------

    async fn get_quota_usage(&self, period: &str) -> Result<Option<(u64, u64)>, GatewayError> {
        let row =
            sqlx::query("SELECT calls_total, calls_rejected FROM quota_usage WHERE period = ?")
                .bind(period)
                .fetch_optional(&self.pool)
                .await?;

        Ok(row.map(|r| {
            let calls: i64 = r.get(0);
            let rejected: i64 = r.get(1);
            (calls.max(0) as u64, rejected.max(0) as u64)
        }))
    }

    async fn upsert_quota_usage(
        &self,
        period: &str,
        calls_total: u64,
        calls_rejected: u64,
    ) -> Result<(), GatewayError> {
        sqlx::query(
            "INSERT INTO quota_usage (period, calls_total, calls_rejected, updated_at) \
             VALUES (?, ?, ?, ?) \
             ON CONFLICT(period) DO UPDATE SET \
               calls_total    = excluded.calls_total, \
               calls_rejected = excluded.calls_rejected, \
               updated_at     = excluded.updated_at",
        )
        .bind(period)
        .bind(calls_total as i64)
        .bind(calls_rejected as i64)
        .bind(Utc::now().to_rfc3339())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    // ---------- logs ----------

    async fn insert_logs(&self, entries: &[LogEntry]) -> Result<(), GatewayError> {
        if entries.is_empty() {
            return Ok(());
        }
        let mut tx = self.pool.begin().await?;
        for e in entries {
            sqlx::query(
                "INSERT INTO logs (id, urn, ts, level, message, service_id, route_id, project_id, \
                 method, path, status, duration_ms, client_ip, metadata, created_at, updated_at) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(e.id.to_string())
            .bind(&e.urn)
            .bind(e.ts.to_rfc3339())
            .bind(&e.level)
            .bind(&e.message)
            .bind(e.service_id.map(|u| u.to_string()))
            .bind(e.route_id.map(|u| u.to_string()))
            .bind(e.project_id.map(|u| u.to_string()))
            .bind(&e.method)
            .bind(&e.path)
            .bind(e.status)
            .bind(e.duration_ms)
            .bind(&e.client_ip)
            .bind(e.metadata.as_ref().map(|v| v.to_string()))
            .bind(e.created_at.to_rfc3339())
            .bind(e.updated_at.to_rfc3339())
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    async fn list_logs(&self, filter: &LogFilter) -> Result<Vec<LogEntry>, GatewayError> {
        let mut sql = String::from(
            "SELECT id, urn, ts, level, message, service_id, route_id, project_id, \
             method, path, status, duration_ms, client_ip, metadata, created_at, updated_at FROM logs WHERE 1=1",
        );
        let mut binds: Vec<String> = Vec::new();
        if let Some(ref sid) = filter.service_id {
            binds.push(sid.to_string());
            sql.push_str(&format!(" AND service_id = ${}", binds.len()));
        }
        if let Some(ref rid) = filter.route_id {
            binds.push(rid.to_string());
            sql.push_str(&format!(" AND route_id = ${}", binds.len()));
        }
        if let Some(ref pid) = filter.project_id {
            binds.push(pid.to_string());
            sql.push_str(&format!(" AND project_id = ${}", binds.len()));
        }
        if let Some(ref level) = filter.level {
            binds.push(level.clone());
            sql.push_str(&format!(" AND level = ${}", binds.len()));
        }
        if let Some(ref search) = filter.search {
            let pat = format!("%{search}%");
            binds.push(pat);
            let idx = binds.len();
            sql.push_str(&format!(
                " AND (message LIKE ${idx} OR path LIKE ${idx} OR client_ip LIKE ${idx})"
            ));
        }
        let limit = filter.limit.unwrap_or(200).min(1000);
        sql.push_str(&format!(" ORDER BY ts DESC LIMIT {limit}"));

        let mut q = sqlx::query(&sql);
        for b in &binds {
            q = q.bind(b);
        }
        let rows = q.fetch_all(&self.pool).await?;
        rows.into_iter().map(row_to_log).collect()
    }

    async fn prune_logs(&self, retention_days: u32) -> Result<u64, GatewayError> {
        let result = sqlx::query(&format!(
            "DELETE FROM logs WHERE ts < datetime('now', '-{retention_days} days')"
        ))
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected())
    }

    // ── Analytics ──

    async fn upsert_metric_bucket(&self, b: &MetricBucket) -> Result<(), GatewayError> {
        sqlx::query(
            "INSERT INTO metric_buckets (id, service_id, route_id, bucket_ts, granularity, \
             total_requests, success_count, error_4xx, error_5xx, total_latency_ms, \
             min_latency_ms, max_latency_ms, bytes_in, bytes_out) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"
        )
        .bind(b.id.to_string())
        .bind(b.service_id.to_string())
        .bind(b.route_id.map(|r| r.to_string()))
        .bind(b.bucket_ts.to_rfc3339())
        .bind(&b.granularity)
        .bind(b.total_requests)
        .bind(b.success_count)
        .bind(b.error_4xx)
        .bind(b.error_5xx)
        .bind(b.total_latency_ms)
        .bind(b.min_latency_ms)
        .bind(b.max_latency_ms)
        .bind(b.bytes_in)
        .bind(b.bytes_out)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn list_metric_buckets(
        &self, service_id: &Uuid, from: DateTime<Utc>, to: DateTime<Utc>, granularity: &str,
    ) -> Result<Vec<MetricBucket>, GatewayError> {
        let rows = sqlx::query(
            "SELECT * FROM metric_buckets WHERE service_id = ? AND bucket_ts >= ? AND bucket_ts <= ? AND granularity = ? ORDER BY bucket_ts"
        )
        .bind(service_id.to_string())
        .bind(from.to_rfc3339())
        .bind(to.to_rfc3339())
        .bind(granularity)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(row_to_metric_bucket).collect()
    }

    async fn upsert_geo_bucket(&self, b: &GeoBucket) -> Result<(), GatewayError> {
        sqlx::query(
            "INSERT INTO geo_buckets (id, service_id, bucket_ts, country_code, city, request_count) \
             VALUES (?, ?, ?, ?, ?, ?)"
        )
        .bind(b.id.to_string())
        .bind(b.service_id.to_string())
        .bind(b.bucket_ts.to_rfc3339())
        .bind(&b.country_code)
        .bind(&b.city)
        .bind(b.request_count)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn list_geo_buckets(
        &self, service_id: &Uuid, from: DateTime<Utc>, to: DateTime<Utc>,
    ) -> Result<Vec<GeoBucket>, GatewayError> {
        let rows = sqlx::query(
            "SELECT * FROM geo_buckets WHERE service_id = ? AND bucket_ts >= ? AND bucket_ts <= ? ORDER BY bucket_ts"
        )
        .bind(service_id.to_string())
        .bind(from.to_rfc3339())
        .bind(to.to_rfc3339())
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(row_to_geo_bucket).collect()
    }

    async fn upsert_rate_limit_bucket(&self, b: &RateLimitBucket) -> Result<(), GatewayError> {
        sqlx::query(
            "INSERT INTO rate_limit_buckets (id, service_id, consumer_id, bucket_ts, allowed, throttled) \
             VALUES (?, ?, ?, ?, ?, ?)"
        )
        .bind(b.id.to_string())
        .bind(b.service_id.to_string())
        .bind(b.consumer_id.map(|c| c.to_string()))
        .bind(b.bucket_ts.to_rfc3339())
        .bind(b.allowed)
        .bind(b.throttled)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn list_rate_limit_buckets(
        &self, service_id: &Uuid, from: DateTime<Utc>, to: DateTime<Utc>,
    ) -> Result<Vec<RateLimitBucket>, GatewayError> {
        let rows = sqlx::query(
            "SELECT * FROM rate_limit_buckets WHERE service_id = ? AND bucket_ts >= ? AND bucket_ts <= ? ORDER BY bucket_ts"
        )
        .bind(service_id.to_string())
        .bind(from.to_rfc3339())
        .bind(to.to_rfc3339())
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(row_to_rate_limit_bucket).collect()
    }

    // ── Probes ──

    async fn list_probes(&self, service_id: &Uuid) -> Result<Vec<Probe>, GatewayError> {
        let rows = sqlx::query("SELECT * FROM probes WHERE service_id = ? ORDER BY created_at")
            .bind(service_id.to_string())
            .fetch_all(&self.pool)
            .await?;
        rows.into_iter().map(row_to_probe).collect()
    }

    async fn get_probe(&self, id: &Uuid) -> Result<Option<Probe>, GatewayError> {
        let row = sqlx::query("SELECT * FROM probes WHERE id = ?")
            .bind(id.to_string())
            .fetch_optional(&self.pool)
            .await?;
        row.map(row_to_probe).transpose()
    }

    async fn create_probe(&self, p: &Probe) -> Result<(), GatewayError> {
        sqlx::query(
            "INSERT INTO probes (id, service_id, route_id, name, method, path, interval_seconds, timeout_ms, expected_status, enabled, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"
        )
        .bind(p.id.to_string())
        .bind(p.service_id.to_string())
        .bind(p.route_id.map(|r| r.to_string()))
        .bind(&p.name)
        .bind(&p.method)
        .bind(&p.path)
        .bind(p.interval_seconds)
        .bind(p.timeout_ms)
        .bind(p.expected_status)
        .bind(p.enabled as i32)
        .bind(p.created_at.to_rfc3339())
        .bind(p.updated_at.to_rfc3339())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn update_probe(&self, id: &Uuid, p: &Probe) -> Result<(), GatewayError> {
        sqlx::query(
            "UPDATE probes SET name=?, method=?, path=?, interval_seconds=?, timeout_ms=?, expected_status=?, enabled=? WHERE id=?"
        )
        .bind(&p.name)
        .bind(&p.method)
        .bind(&p.path)
        .bind(p.interval_seconds)
        .bind(p.timeout_ms)
        .bind(p.expected_status)
        .bind(p.enabled as i32)
        .bind(id.to_string())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn delete_probe(&self, id: &Uuid) -> Result<bool, GatewayError> {
        let r = sqlx::query("DELETE FROM probes WHERE id = ?")
            .bind(id.to_string())
            .execute(&self.pool)
            .await?;
        Ok(r.rows_affected() > 0)
    }

    async fn append_probe_result(&self, r: &ProbeResult) -> Result<(), GatewayError> {
        sqlx::query(
            "INSERT INTO probe_results (id, probe_id, ts, status, latency_ms, healthy, error) VALUES (?, ?, ?, ?, ?, ?, ?)"
        )
        .bind(r.id.to_string())
        .bind(r.probe_id.to_string())
        .bind(r.ts.to_rfc3339())
        .bind(r.status)
        .bind(r.latency_ms)
        .bind(r.healthy as i32)
        .bind(&r.error)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn list_probe_results(&self, probe_id: &Uuid, limit: u32) -> Result<Vec<ProbeResult>, GatewayError> {
        let rows = sqlx::query("SELECT * FROM probe_results WHERE probe_id = ? ORDER BY ts DESC LIMIT ?")
            .bind(probe_id.to_string())
            .bind(limit)
            .fetch_all(&self.pool)
            .await?;
        rows.into_iter().map(row_to_probe_result).collect()
    }

    // ── Snapshots ──

    async fn list_snapshots(&self, service_id: &Uuid) -> Result<Vec<Snapshot>, GatewayError> {
        let rows = sqlx::query("SELECT * FROM snapshots WHERE service_id = ? ORDER BY created_at DESC")
            .bind(service_id.to_string())
            .fetch_all(&self.pool)
            .await?;
        rows.into_iter().map(row_to_snapshot).collect()
    }

    async fn get_snapshot(&self, id: &Uuid) -> Result<Option<Snapshot>, GatewayError> {
        let row = sqlx::query("SELECT * FROM snapshots WHERE id = ?")
            .bind(id.to_string())
            .fetch_optional(&self.pool)
            .await?;
        row.map(row_to_snapshot).transpose()
    }

    async fn create_snapshot(&self, s: &Snapshot) -> Result<(), GatewayError> {
        let data_str = serde_json::to_string(&s.data).unwrap_or_default();
        sqlx::query("INSERT INTO snapshots (id, service_id, name, data, created_at) VALUES (?, ?, ?, ?, ?)")
            .bind(s.id.to_string())
            .bind(s.service_id.to_string())
            .bind(&s.name)
            .bind(&data_str)
            .bind(s.created_at.to_rfc3339())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn delete_snapshot(&self, id: &Uuid) -> Result<bool, GatewayError> {
        let r = sqlx::query("DELETE FROM snapshots WHERE id = ?")
            .bind(id.to_string())
            .execute(&self.pool)
            .await?;
        Ok(r.rows_affected() > 0)
    }

    // ── Alerts ──

    async fn list_alert_rules(&self, service_id: &Uuid) -> Result<Vec<AlertRule>, GatewayError> {
        let rows = sqlx::query("SELECT * FROM alert_rules WHERE service_id = ? ORDER BY created_at")
            .bind(service_id.to_string())
            .fetch_all(&self.pool)
            .await?;
        rows.into_iter().map(row_to_alert_rule).collect()
    }

    async fn get_alert_rule(&self, id: &Uuid) -> Result<Option<AlertRule>, GatewayError> {
        let row = sqlx::query("SELECT * FROM alert_rules WHERE id = ?")
            .bind(id.to_string())
            .fetch_optional(&self.pool)
            .await?;
        row.map(row_to_alert_rule).transpose()
    }

    async fn create_alert_rule(&self, r: &AlertRule) -> Result<(), GatewayError> {
        let channels_str = serde_json::to_string(&r.channels).unwrap_or_default();
        sqlx::query(
            "INSERT INTO alert_rules (id, service_id, name, metric, operator, threshold, window_seconds, cooldown_seconds, channels, enabled, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"
        )
        .bind(r.id.to_string())
        .bind(r.service_id.to_string())
        .bind(&r.name)
        .bind(&r.metric)
        .bind(&r.operator)
        .bind(r.threshold)
        .bind(r.window_seconds)
        .bind(r.cooldown_seconds)
        .bind(&channels_str)
        .bind(r.enabled as i32)
        .bind(r.created_at.to_rfc3339())
        .bind(r.updated_at.to_rfc3339())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn update_alert_rule(&self, id: &Uuid, r: &AlertRule) -> Result<(), GatewayError> {
        let channels_str = serde_json::to_string(&r.channels).unwrap_or_default();
        sqlx::query(
            "UPDATE alert_rules SET name=?, metric=?, operator=?, threshold=?, window_seconds=?, cooldown_seconds=?, channels=?, enabled=? WHERE id=?"
        )
        .bind(&r.name)
        .bind(&r.metric)
        .bind(&r.operator)
        .bind(r.threshold)
        .bind(r.window_seconds)
        .bind(r.cooldown_seconds)
        .bind(&channels_str)
        .bind(r.enabled as i32)
        .bind(id.to_string())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn delete_alert_rule(&self, id: &Uuid) -> Result<bool, GatewayError> {
        let r = sqlx::query("DELETE FROM alert_rules WHERE id = ?")
            .bind(id.to_string())
            .execute(&self.pool)
            .await?;
        Ok(r.rows_affected() > 0)
    }

    async fn list_alert_events(&self, service_id: &Uuid, limit: u32) -> Result<Vec<AlertEvent>, GatewayError> {
        let rows = sqlx::query("SELECT * FROM alert_events WHERE service_id = ? ORDER BY ts DESC LIMIT ?")
            .bind(service_id.to_string())
            .bind(limit)
            .fetch_all(&self.pool)
            .await?;
        rows.into_iter().map(row_to_alert_event).collect()
    }

    async fn create_alert_event(&self, e: &AlertEvent) -> Result<(), GatewayError> {
        sqlx::query(
            "INSERT INTO alert_events (id, rule_id, service_id, ts, metric, value, threshold, message, resolved) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)"
        )
        .bind(e.id.to_string())
        .bind(e.rule_id.to_string())
        .bind(e.service_id.to_string())
        .bind(e.ts.to_rfc3339())
        .bind(&e.metric)
        .bind(e.value)
        .bind(e.threshold)
        .bind(&e.message)
        .bind(e.resolved as i32)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn resolve_alert_event(&self, id: &Uuid) -> Result<bool, GatewayError> {
        let r = sqlx::query("UPDATE alert_events SET resolved = 1, resolved_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') WHERE id = ? AND resolved = 0")
            .bind(id.to_string())
            .execute(&self.pool)
            .await?;
        Ok(r.rows_affected() > 0)
    }

    // ── IP Rules ──

    async fn list_ip_rules(&self, service_id: &Uuid) -> Result<Vec<IpRule>, GatewayError> {
        let rows = sqlx::query("SELECT * FROM ip_rules WHERE service_id = ? ORDER BY priority, created_at")
            .bind(service_id.to_string())
            .fetch_all(&self.pool)
            .await?;
        rows.into_iter().map(row_to_ip_rule).collect()
    }

    async fn get_ip_rule(&self, id: &Uuid) -> Result<Option<IpRule>, GatewayError> {
        let row = sqlx::query("SELECT * FROM ip_rules WHERE id = ?")
            .bind(id.to_string())
            .fetch_optional(&self.pool)
            .await?;
        row.map(row_to_ip_rule).transpose()
    }

    async fn create_ip_rule(&self, r: &IpRule) -> Result<(), GatewayError> {
        sqlx::query(
            "INSERT INTO ip_rules (id, service_id, route_id, cidr, action, description, priority, enabled, hits, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"
        )
        .bind(r.id.to_string())
        .bind(r.service_id.to_string())
        .bind(r.route_id.map(|x| x.to_string()))
        .bind(&r.cidr)
        .bind(&r.action)
        .bind(&r.description)
        .bind(r.priority)
        .bind(r.enabled as i32)
        .bind(r.hits)
        .bind(r.created_at.to_rfc3339())
        .bind(r.updated_at.to_rfc3339())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn update_ip_rule(&self, id: &Uuid, r: &IpRule) -> Result<(), GatewayError> {
        sqlx::query(
            "UPDATE ip_rules SET cidr=?, action=?, description=?, priority=?, enabled=? WHERE id=?"
        )
        .bind(&r.cidr)
        .bind(&r.action)
        .bind(&r.description)
        .bind(r.priority)
        .bind(r.enabled as i32)
        .bind(id.to_string())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn delete_ip_rule(&self, id: &Uuid) -> Result<bool, GatewayError> {
        let r = sqlx::query("DELETE FROM ip_rules WHERE id = ?")
            .bind(id.to_string())
            .execute(&self.pool)
            .await?;
        Ok(r.rows_affected() > 0)
    }

    // ── Error Pages ──

    async fn list_error_pages(&self, service_id: &Uuid) -> Result<Vec<ErrorPage>, GatewayError> {
        let rows = sqlx::query("SELECT * FROM error_pages WHERE service_id = ? ORDER BY status_code")
            .bind(service_id.to_string())
            .fetch_all(&self.pool)
            .await?;
        rows.into_iter().map(row_to_error_page).collect()
    }

    async fn get_error_page(&self, id: &Uuid) -> Result<Option<ErrorPage>, GatewayError> {
        let row = sqlx::query("SELECT * FROM error_pages WHERE id = ?")
            .bind(id.to_string())
            .fetch_optional(&self.pool)
            .await?;
        row.map(row_to_error_page).transpose()
    }

    async fn create_error_page(&self, p: &ErrorPage) -> Result<(), GatewayError> {
        sqlx::query(
            "INSERT INTO error_pages (id, service_id, status_code, content_type, body, enabled, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)"
        )
        .bind(p.id.to_string())
        .bind(p.service_id.to_string())
        .bind(p.status_code)
        .bind(&p.content_type)
        .bind(&p.body)
        .bind(p.enabled as i32)
        .bind(p.created_at.to_rfc3339())
        .bind(p.updated_at.to_rfc3339())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn update_error_page(&self, id: &Uuid, p: &ErrorPage) -> Result<(), GatewayError> {
        sqlx::query(
            "UPDATE error_pages SET status_code=?, content_type=?, body=?, enabled=? WHERE id=?"
        )
        .bind(p.status_code)
        .bind(&p.content_type)
        .bind(&p.body)
        .bind(p.enabled as i32)
        .bind(id.to_string())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn delete_error_page(&self, id: &Uuid) -> Result<bool, GatewayError> {
        let r = sqlx::query("DELETE FROM error_pages WHERE id = ?")
            .bind(id.to_string())
            .execute(&self.pool)
            .await?;
        Ok(r.rows_affected() > 0)
    }

    // ── Consumer Groups ──

    async fn list_consumer_groups(&self, service_id: &Uuid) -> Result<Vec<ConsumerGroup>, GatewayError> {
        let rows = sqlx::query("SELECT * FROM consumer_groups WHERE service_id = ? ORDER BY created_at")
            .bind(service_id.to_string())
            .fetch_all(&self.pool)
            .await?;
        rows.into_iter().map(row_to_consumer_group).collect()
    }

    async fn get_consumer_group(&self, id: &Uuid) -> Result<Option<ConsumerGroup>, GatewayError> {
        let row = sqlx::query("SELECT * FROM consumer_groups WHERE id = ?")
            .bind(id.to_string())
            .fetch_optional(&self.pool)
            .await?;
        row.map(row_to_consumer_group).transpose()
    }

    async fn create_consumer_group(&self, g: &ConsumerGroup) -> Result<(), GatewayError> {
        sqlx::query(
            "INSERT INTO consumer_groups (id, service_id, name, description, rate_limit, rate_window_seconds, burst_limit, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)"
        )
        .bind(g.id.to_string())
        .bind(g.service_id.to_string())
        .bind(&g.name)
        .bind(&g.description)
        .bind(g.rate_limit)
        .bind(g.rate_window_seconds)
        .bind(g.burst_limit)
        .bind(g.created_at.to_rfc3339())
        .bind(g.updated_at.to_rfc3339())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn update_consumer_group(&self, id: &Uuid, g: &ConsumerGroup) -> Result<(), GatewayError> {
        sqlx::query(
            "UPDATE consumer_groups SET name=?, description=?, rate_limit=?, rate_window_seconds=?, burst_limit=? WHERE id=?"
        )
        .bind(&g.name)
        .bind(&g.description)
        .bind(g.rate_limit)
        .bind(g.rate_window_seconds)
        .bind(g.burst_limit)
        .bind(id.to_string())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn delete_consumer_group(&self, id: &Uuid) -> Result<bool, GatewayError> {
        let r = sqlx::query("DELETE FROM consumer_groups WHERE id = ?")
            .bind(id.to_string())
            .execute(&self.pool)
            .await?;
        Ok(r.rows_affected() > 0)
    }

    async fn list_consumer_group_members(&self, group_id: &Uuid) -> Result<Vec<ConsumerGroupMember>, GatewayError> {
        let rows = sqlx::query("SELECT * FROM consumer_group_members WHERE group_id = ? ORDER BY joined_at")
            .bind(group_id.to_string())
            .fetch_all(&self.pool)
            .await?;
        rows.into_iter().map(row_to_consumer_group_member).collect()
    }

    async fn add_consumer_group_member(&self, m: &ConsumerGroupMember) -> Result<(), GatewayError> {
        sqlx::query(
            "INSERT OR IGNORE INTO consumer_group_members (group_id, consumer_id, joined_at) VALUES (?, ?, ?)"
        )
        .bind(m.group_id.to_string())
        .bind(m.consumer_id.to_string())
        .bind(m.joined_at.to_rfc3339())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn remove_consumer_group_member(&self, group_id: &Uuid, consumer_id: &Uuid) -> Result<bool, GatewayError> {
        let r = sqlx::query("DELETE FROM consumer_group_members WHERE group_id = ? AND consumer_id = ?")
            .bind(group_id.to_string())
            .bind(consumer_id.to_string())
            .execute(&self.pool)
            .await?;
        Ok(r.rows_affected() > 0)
    }
}

/// Shared seed logic — both repos express the same flow against the trait
/// surface, so we factor it out.
///
/// Only removes rows whose `source = 'config'`. Admin-API rows
/// (`source = 'api'`) are untouched, so dynamic state survives
/// `--reseed` and normal restarts with `seed_from_config = true`.
/// Idempotent config-to-DB sync. Services are upserted by name so their
/// UUIDs are stable across restarts — routes, plugins, and any admin-API
/// state that references a service by ID won't break.
///
/// Routes are matched by (hosts, service_id) and plugins by (name, scope).
/// Rows created here are tagged `source = 'config'`; admin-API rows
/// (`source = 'api'`) are never touched.
pub(crate) async fn seed_from_config_generic<R: Repository + ?Sized>(
    repo: &R,
    config: &GatewayConfig,
) -> Result<(), GatewayError> {
    // Phase 1: delete config-sourced rows that are NO LONGER in config.toml.
    // We collect the names/hosts we're about to upsert, then remove stale
    // config-sourced rows that don't match.
    // Delete config-sourced rows that are no longer in config.toml,
    // then we'll upsert the current config entries below.
    repo.clear_config_seed().await?;

    let mut service_ids = Vec::new();
    let mut route_ids = Vec::new();
    let mut plugin_ids = Vec::new();

    // Phase 2: upsert services by name. If a service with the same name
    // already exists (from admin API or a previous seed), reuse its UUID
    // and just update the url/retries/timeout.
    let mut services_by_name = std::collections::HashMap::new();
    for svc_cfg in &config.services {
        if let Some(existing) = repo.get_service_by_name(&svc_cfg.name).await? {
            // Service exists — update in place, preserving UUID.
            let updated = Service {
                id: existing.id,
                name: svc_cfg.name.clone(),
                url: svc_cfg.url.clone(),
                retries: svc_cfg.retries,
                connect_timeout_ms: svc_cfg.connect_timeout_ms,
                lb_strategy: existing.lb_strategy.clone(),
                service_type: svc_cfg.service_type.clone().unwrap_or_else(|| existing.service_type.clone()),
                document_root: svc_cfg.document_root.clone().or_else(|| existing.document_root.clone()),
                index_file: svc_cfg.index_file.clone().unwrap_or_else(|| existing.index_file.clone()),
                spa_fallback: svc_cfg.spa_fallback.unwrap_or(existing.spa_fallback),
                tls_verify: existing.tls_verify,
                tls_client_cert_path: existing.tls_client_cert_path.clone(),
                tls_client_key_path: existing.tls_client_key_path.clone(),
                tls_ca_path: existing.tls_ca_path.clone(),
                lb_hash_header: existing.lb_hash_header.clone(),
                lb_hash_query_param: existing.lb_hash_query_param.clone(),
                flush_interval_ms: existing.flush_interval_ms,
                stream_timeout_ms: existing.stream_timeout_ms,
                stream_close_delay_ms: existing.stream_close_delay_ms,
                pass_host_header: existing.pass_host_header,
                drain_timeout_ms: None,
                project_id: existing.project_id,
                domain: existing.domain.clone(),
                subdomain: existing.subdomain.clone(),
                created_at: existing.created_at,
                updated_at: Utc::now(),
            };
            repo.update_service(&existing.id, &updated).await?;
            service_ids.push(existing.id);
            services_by_name.insert(svc_cfg.name.clone(), existing.id);
            tracing::debug!(name = %svc_cfg.name, id = %existing.id, "config seed: updated existing service");
        } else {
            // New service — create with a fresh UUID.
            let mut service = Service::new(
                svc_cfg.name.clone(),
                svc_cfg.url.clone(),
                svc_cfg.retries,
                svc_cfg.connect_timeout_ms,
            );
            if let Some(t) = &svc_cfg.service_type {
                service.service_type = t.clone();
            }
            if let Some(root) = &svc_cfg.document_root {
                service.document_root = Some(root.clone());
            }
            if let Some(idx) = &svc_cfg.index_file {
                service.index_file = idx.clone();
            }
            if let Some(spa) = svc_cfg.spa_fallback {
                service.spa_fallback = spa;
            }
            repo.create_service(&service).await?;
            service_ids.push(service.id);
            services_by_name.insert(service.name.clone(), service.id);
            tracing::debug!(name = %svc_cfg.name, id = %service.id, "config seed: created new service");
        }
    }

    // Phase 3: upsert routes. Match by (hosts, service_id) to avoid
    // duplicates when the same route is seeded on every restart.
    let existing_routes = repo.list_routes().await?;
    for rt_cfg in &config.routes {
        let Some(service_id) = services_by_name.get(&rt_cfg.service).copied() else {
            tracing::warn!(
                service = %rt_cfg.service,
                "skipping seed route; referenced service not in config"
            );
            continue;
        };

        // Check if a route with the same hosts+service already exists.
        let already = existing_routes.iter().find(|r| {
            r.service_id == service_id && r.hosts == rt_cfg.hosts
        });

        if let Some(existing) = already {
            // Update in place.
            let updated = Route {
                id: existing.id,
                paths: rt_cfg.paths.clone(),
                methods: rt_cfg.methods.clone(),
                hosts: rt_cfg.hosts.clone(),
                service_id,
                strip_path: rt_cfg.strip_path,
                buffer_request: false,
                path_rewrite_pattern: None,
                path_rewrite_replacement: None,
                headers: std::collections::HashMap::new(),
                query_params: std::collections::HashMap::new(),
                path_regexes: Vec::new(),
                read_timeout_ms: None,
                write_timeout_ms: None,
                idle_timeout_ms: None,
                retry_on_status: Vec::new(),
                matchers: Vec::new(),
                matcher_refs: Vec::new(),
                handle_blocks: Vec::new(),
                try_files: None,
                uri_directives: Vec::new(),
                intercept_rules: Vec::new(),
                push: None,
                static_response: None,
                health_gate: false,
                created_at: existing.created_at,
                updated_at: Utc::now(),
            };
            repo.update_route(&existing.id, &updated).await?;
            route_ids.push(existing.id);
        } else {
            let route = Route::new(
                rt_cfg.paths.clone(),
                rt_cfg.methods.clone(),
                rt_cfg.hosts.clone(),
                service_id,
                rt_cfg.strip_path,
            );
            repo.create_route(&route).await?;
            route_ids.push(route.id);
        }
    }

    // Phase 4: upsert global plugins by name.
    let existing_plugins = repo.list_plugins().await?;
    for plugin_cfg in &config.plugins {
        let config_value =
            serde_json::to_value(&plugin_cfg.config).unwrap_or(serde_json::Value::Null);

        // Match global plugins (no service/route scope) by name.
        let already = existing_plugins.iter().find(|p| {
            p.name == plugin_cfg.name && p.service_id.is_none() && p.route_id.is_none()
        });

        if let Some(existing) = already {
            let updated = PluginInstance {
                id: existing.id,
                name: plugin_cfg.name.clone(),
                service_id: None,
                route_id: None,
                enabled: plugin_cfg.enabled,
                config: config_value,
                created_at: existing.created_at,
                updated_at: Utc::now(),
            };
            repo.update_plugin(&existing.id, &updated).await?;
            plugin_ids.push(existing.id);
        } else {
            let now = Utc::now();
            let plugin = PluginInstance {
                id: Uuid::new_v4(),
                name: plugin_cfg.name.clone(),
                service_id: None,
                route_id: None,
                enabled: plugin_cfg.enabled,
                config: config_value,
                created_at: now,
                updated_at: now,
            };
            repo.create_plugin(&plugin).await?;
            plugin_ids.push(plugin.id);
        }
    }

    repo.mark_config_source(&service_ids, &route_ids, &plugin_ids)
        .await?;

    Ok(())
}

// ---------- row mappers ----------

fn parse_uuid(s: &str) -> Result<Uuid, GatewayError> {
    Uuid::from_str(s).map_err(|e| GatewayError::Internal(format!("invalid uuid in db: {e}")))
}

fn parse_dt(s: &str) -> Result<DateTime<Utc>, GatewayError> {
    DateTime::parse_from_rfc3339(s)
        .map(|dt| dt.with_timezone(&Utc))
        .or_else(|_| {
            chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S")
                .map(|ndt| ndt.and_utc())
        })
        .map_err(|e| GatewayError::Internal(format!("invalid timestamp in db: {e}")))
}

fn row_to_service(row: sqlx::sqlite::SqliteRow) -> Result<Service, GatewayError> {
    let id: String = row.try_get("id")?;
    let retries: i64 = row.try_get("retries")?;
    let connect_timeout_ms: i64 = row.try_get("connect_timeout_ms")?;
    let created_at: String = row.try_get("created_at")?;
    let updated_at: String = row.try_get("updated_at")?;

    let lb_strategy: String = row
        .try_get("lb_strategy")
        .unwrap_or_else(|_| "round-robin".to_string());

    let service_type: String = row
        .try_get("service_type")
        .unwrap_or_else(|_| "api".to_string());

    let document_root: Option<String> = row
        .try_get::<Option<String>, _>("document_root")
        .ok()
        .flatten();
    let index_file: String = row
        .try_get("index_file")
        .unwrap_or_else(|_| "index.html".to_string());
    let spa_fallback: bool = row
        .try_get::<i64, _>("spa_fallback")
        .map(|v| v != 0)
        .unwrap_or(false);

    // TLS columns are introduced in migration 0006. `try_get` failure
    // (e.g. running against an out-of-date schema) falls back to safe
    // defaults: verify on, no client cert, no extra CA.
    let tls_verify: bool = row
        .try_get::<i64, _>("tls_verify")
        .map(|v| v != 0)
        .unwrap_or(true);
    let tls_client_cert_path: Option<String> = row
        .try_get::<Option<String>, _>("tls_client_cert_path")
        .ok()
        .flatten();
    let tls_client_key_path: Option<String> = row
        .try_get::<Option<String>, _>("tls_client_key_path")
        .ok()
        .flatten();
    let tls_ca_path: Option<String> = row
        .try_get::<Option<String>, _>("tls_ca_path")
        .ok()
        .flatten();

    Ok(Service {
        id: parse_uuid(&id)?,
        name: row.try_get("name")?,
        url: row.try_get("url")?,
        retries: retries as u32,
        connect_timeout_ms: connect_timeout_ms as u64,
        lb_strategy,
        service_type,
        document_root,
        index_file,
        spa_fallback,
        tls_verify,
        tls_client_cert_path,
        tls_client_key_path,
        tls_ca_path,
        lb_hash_header: row.try_get::<Option<String>, _>("lb_hash_header").ok().flatten(),
        lb_hash_query_param: row.try_get::<Option<String>, _>("lb_hash_query_param").ok().flatten(),
        flush_interval_ms: row.try_get::<Option<i64>, _>("flush_interval_ms").ok().flatten(),
        stream_timeout_ms: row.try_get::<Option<i64>, _>("stream_timeout_ms").ok().flatten().map(|v| v as u64),
        stream_close_delay_ms: row.try_get::<Option<i64>, _>("stream_close_delay_ms").ok().flatten().map(|v| v as u64),
        pass_host_header: row.try_get::<Option<bool>, _>("pass_host_header").ok().flatten(),
        drain_timeout_ms: row.try_get::<Option<i64>, _>("drain_timeout_ms").ok().flatten().map(|v| v as u64),
        project_id: row
            .try_get::<Option<String>, _>("project_id")
            .ok()
            .flatten()
            .and_then(|s| Uuid::from_str(&s).ok()),
        domain: row.try_get("domain").unwrap_or_default(),
        subdomain: row.try_get("subdomain").unwrap_or_default(),
        created_at: parse_dt(&created_at)?,
        updated_at: parse_dt(&updated_at)?,
    })
}

fn row_to_project(row: sqlx::sqlite::SqliteRow) -> Result<Project, GatewayError> {
    let id: String = row.try_get("id")?;
    let created_at: String = row.try_get("created_at")?;
    let updated_at: String = row.try_get("updated_at")?;
    Ok(Project {
        id: parse_uuid(&id)?,
        name: row.try_get("name")?,
        description: row.try_get("description")?,
        created_at: parse_dt(&created_at)?,
        updated_at: parse_dt(&updated_at)?,
    })
}

fn row_to_target(row: sqlx::sqlite::SqliteRow) -> Result<Target, GatewayError> {
    let id: String = row.try_get("id")?;
    let service_id: String = row.try_get("service_id")?;
    let weight: i64 = row.try_get("weight")?;
    let enabled: i64 = row.try_get("enabled")?;
    let zone: Option<String> = row.try_get("zone").ok();
    let state_str: String = row.try_get("state").unwrap_or_else(|_| "active".to_string());
    let created_at: String = row.try_get("created_at")?;
    let updated_at: String = row.try_get("updated_at")?;

    Ok(Target {
        id: parse_uuid(&id)?,
        service_id: parse_uuid(&service_id)?,
        url: row.try_get("url")?,
        weight: weight as u32,
        enabled: enabled != 0,
        zone,
        state: crate::models::TargetLifecycle::from_str(&state_str),
        created_at: parse_dt(&created_at)?,
        updated_at: parse_dt(&updated_at)?,
    })
}

fn row_to_route(row: sqlx::sqlite::SqliteRow) -> Result<Route, GatewayError> {
    let id: String = row.try_get("id")?;
    let service_id: String = row.try_get("service_id")?;
    let paths: String = row.try_get("paths")?;
    let methods: String = row.try_get("methods")?;
    let hosts: String = row.try_get("hosts").unwrap_or_else(|_| "[]".to_string());
    let strip_path: i64 = row.try_get("strip_path")?;
    let created_at: String = row.try_get("created_at")?;
    let updated_at: String = row.try_get("updated_at")?;

    Ok(Route {
        id: parse_uuid(&id)?,
        service_id: parse_uuid(&service_id)?,
        paths: serde_json::from_str(&paths)
            .map_err(|e| GatewayError::Internal(format!("invalid paths json: {e}")))?,
        methods: serde_json::from_str(&methods)
            .map_err(|e| GatewayError::Internal(format!("invalid methods json: {e}")))?,
        hosts: serde_json::from_str(&hosts).unwrap_or_default(),
        strip_path: strip_path != 0,
        buffer_request: false,
        path_rewrite_pattern: None,
        path_rewrite_replacement: None,
        headers: std::collections::HashMap::new(),
        query_params: std::collections::HashMap::new(),
        path_regexes: Vec::new(),
        read_timeout_ms: None,
        write_timeout_ms: None,
        idle_timeout_ms: None,
        retry_on_status: Vec::new(),
        matchers: Vec::new(),
        matcher_refs: Vec::new(),
        handle_blocks: Vec::new(),
        try_files: None,
        uri_directives: Vec::new(),
        intercept_rules: Vec::new(),
        push: None,
        static_response: None,
        health_gate: false,
        created_at: parse_dt(&created_at)?,
        updated_at: parse_dt(&updated_at)?,
    })
}

fn row_to_plugin(row: sqlx::sqlite::SqliteRow) -> Result<PluginInstance, GatewayError> {
    let id: String = row.try_get("id")?;
    let service_id: Option<String> = row.try_get("service_id").ok();
    let route_id: Option<String> = row.try_get("route_id").ok();
    let enabled: i64 = row.try_get("enabled")?;
    let config: String = row.try_get("config")?;
    let created_at: String = row.try_get("created_at")?;
    let updated_at: String = row.try_get("updated_at")?;

    let service_id = match service_id.filter(|s| !s.is_empty()) {
        Some(s) => Some(parse_uuid(&s)?),
        None => None,
    };
    let route_id = match route_id.filter(|s| !s.is_empty()) {
        Some(s) => Some(parse_uuid(&s)?),
        None => None,
    };

    Ok(PluginInstance {
        id: parse_uuid(&id)?,
        name: row.try_get("name")?,
        service_id,
        route_id,
        enabled: enabled != 0,
        config: serde_json::from_str(&config)
            .map_err(|e| GatewayError::Internal(format!("invalid plugin config json: {e}")))?,
        created_at: parse_dt(&created_at)?,
        updated_at: parse_dt(&updated_at)?,
    })
}

fn row_to_admin_token(row: sqlx::sqlite::SqliteRow) -> Result<AdminToken, GatewayError> {
    let id: String = row.try_get("id")?;
    let role: String = row.try_get("role")?;
    let created_at: String = row.try_get("created_at")?;
    let expires_at: Option<String> = row.try_get("expires_at")?;
    let last_used_at: Option<String> = row.try_get("last_used_at")?;
    Ok(AdminToken {
        id: parse_uuid(&id)?,
        name: row.try_get("name")?,
        role: Role::from_str(&role)
            .map_err(|e| GatewayError::Internal(format!("invalid role in db: {e}")))?,
        created_at: parse_dt(&created_at)?,
        expires_at: expires_at.map(|s| parse_dt(&s)).transpose()?,
        last_used_at: last_used_at.map(|s| parse_dt(&s)).transpose()?,
    })
}

fn row_to_audit(row: sqlx::sqlite::SqliteRow) -> Result<AuditEntry, GatewayError> {
    let id: i64 = row.try_get("id")?;
    let ts: String = row.try_get("ts")?;
    let actor_id: Option<String> = row.try_get("actor_id")?;
    let diff: Option<String> = row.try_get("diff")?;
    let actor_id = actor_id
        .filter(|s| !s.is_empty())
        .map(|s| parse_uuid(&s))
        .transpose()?;
    let diff = match diff {
        Some(s) if !s.is_empty() => Some(
            serde_json::from_str(&s)
                .map_err(|e| GatewayError::Internal(format!("invalid audit diff json: {e}")))?,
        ),
        _ => None,
    };
    Ok(AuditEntry {
        id,
        ts: parse_dt(&ts)?,
        actor_id,
        actor_name: row.try_get("actor_name").ok(),
        actor_role: row.try_get("actor_role").ok(),
        action: row.try_get("action")?,
        resource_type: row.try_get("resource_type")?,
        resource_id: row.try_get("resource_id").ok(),
        diff,
        hash: row.try_get("hash")?,
        prev_hash: row.try_get("prev_hash").ok(),
    })
}

fn row_to_recording_index(row: sqlx::sqlite::SqliteRow) -> Result<RecordingIndexRow, GatewayError> {
    let id: String = row.try_get("id")?;
    let ts: String = row.try_get("ts")?;
    let status: i64 = row.try_get("status")?;
    let duration_ms: i64 = row.try_get("duration_ms")?;
    let service_id: Option<String> = row.try_get("service_id").ok().flatten();
    let route_id: Option<String> = row.try_get("route_id").ok().flatten();
    let consumer: Option<String> = row.try_get("consumer").ok().flatten();
    let bytes_request: i64 = row.try_get("bytes_request")?;
    let bytes_response: i64 = row.try_get("bytes_response")?;

    let service_id = match service_id.filter(|s| !s.is_empty()) {
        Some(s) => Some(parse_uuid(&s)?),
        None => None,
    };
    let route_id = match route_id.filter(|s| !s.is_empty()) {
        Some(s) => Some(parse_uuid(&s)?),
        None => None,
    };

    Ok(RecordingIndexRow {
        id,
        ts: parse_dt(&ts)?,
        method: row.try_get("method")?,
        path: row.try_get("path")?,
        status: status as u16,
        duration_ms: duration_ms as u64,
        service_id,
        route_id,
        consumer,
        bytes_request: bytes_request as u64,
        bytes_response: bytes_response as u64,
    })
}

fn row_to_consumer(row: sqlx::sqlite::SqliteRow) -> Result<Consumer, GatewayError> {
    let id: String = row.try_get("id")?;
    let custom_id: Option<String> = row.try_get("custom_id")?;
    let api_key: Option<String> = row.try_get("api_key")?;
    let created_at: String = row.try_get("created_at")?;
    let updated_at: String = row.try_get("updated_at")?;

    Ok(Consumer {
        id: parse_uuid(&id)?,
        username: row.try_get("username")?,
        custom_id,
        api_key,
        password_hash: None,
        created_at: parse_dt(&created_at)?,
        updated_at: parse_dt(&updated_at)?,
    })
}

fn row_to_log(row: sqlx::sqlite::SqliteRow) -> Result<LogEntry, GatewayError> {
    let id: String = row.try_get("id")?;
    let ts: String = row.try_get("ts")?;
    let service_id: Option<String> = row.try_get("service_id")?;
    let route_id: Option<String> = row.try_get("route_id")?;
    let project_id: Option<String> = row.try_get("project_id")?;
    let metadata: Option<String> = row.try_get("metadata")?;
    let created_at: String = row.try_get("created_at")?;
    let updated_at: String = row.try_get("updated_at")?;
    Ok(LogEntry {
        id: parse_uuid(&id)?,
        urn: row.try_get("urn")?,
        ts: parse_dt(&ts)?,
        level: row.try_get("level")?,
        message: row.try_get("message")?,
        service_id: service_id.as_deref().and_then(|s| Uuid::from_str(s).ok()),
        route_id: route_id.as_deref().and_then(|s| Uuid::from_str(s).ok()),
        project_id: project_id.as_deref().and_then(|s| Uuid::from_str(s).ok()),
        method: row.try_get("method")?,
        path: row.try_get("path")?,
        status: row.try_get("status")?,
        duration_ms: row.try_get("duration_ms")?,
        client_ip: row.try_get("client_ip")?,
        metadata: metadata.and_then(|s| serde_json::from_str(&s).ok()),
        created_at: parse_dt(&created_at)?,
        updated_at: parse_dt(&updated_at)?,
    })
}

fn row_to_metric_bucket(row: sqlx::sqlite::SqliteRow) -> Result<MetricBucket, GatewayError> {
    Ok(MetricBucket {
        id: parse_uuid(&row.try_get::<String, _>("id")?)?,
        service_id: parse_uuid(&row.try_get::<String, _>("service_id")?)?,
        route_id: row.try_get::<Option<String>, _>("route_id")?.and_then(|s| Uuid::from_str(&s).ok()),
        bucket_ts: parse_dt(&row.try_get::<String, _>("bucket_ts")?)?,
        granularity: row.try_get("granularity")?,
        total_requests: row.try_get("total_requests")?,
        success_count: row.try_get("success_count")?,
        error_4xx: row.try_get("error_4xx")?,
        error_5xx: row.try_get("error_5xx")?,
        total_latency_ms: row.try_get("total_latency_ms")?,
        min_latency_ms: row.try_get("min_latency_ms")?,
        max_latency_ms: row.try_get("max_latency_ms")?,
        bytes_in: row.try_get("bytes_in")?,
        bytes_out: row.try_get("bytes_out")?,
    })
}

fn row_to_geo_bucket(row: sqlx::sqlite::SqliteRow) -> Result<GeoBucket, GatewayError> {
    Ok(GeoBucket {
        id: parse_uuid(&row.try_get::<String, _>("id")?)?,
        service_id: parse_uuid(&row.try_get::<String, _>("service_id")?)?,
        bucket_ts: parse_dt(&row.try_get::<String, _>("bucket_ts")?)?,
        country_code: row.try_get("country_code")?,
        city: row.try_get("city")?,
        request_count: row.try_get("request_count")?,
    })
}

fn row_to_rate_limit_bucket(row: sqlx::sqlite::SqliteRow) -> Result<RateLimitBucket, GatewayError> {
    Ok(RateLimitBucket {
        id: parse_uuid(&row.try_get::<String, _>("id")?)?,
        service_id: parse_uuid(&row.try_get::<String, _>("service_id")?)?,
        consumer_id: row.try_get::<Option<String>, _>("consumer_id")?.and_then(|s| Uuid::from_str(&s).ok()),
        bucket_ts: parse_dt(&row.try_get::<String, _>("bucket_ts")?)?,
        allowed: row.try_get("allowed")?,
        throttled: row.try_get("throttled")?,
    })
}

fn row_to_probe(row: sqlx::sqlite::SqliteRow) -> Result<Probe, GatewayError> {
    Ok(Probe {
        id: parse_uuid(&row.try_get::<String, _>("id")?)?,
        service_id: parse_uuid(&row.try_get::<String, _>("service_id")?)?,
        route_id: row.try_get::<Option<String>, _>("route_id")?.and_then(|s| Uuid::from_str(&s).ok()),
        name: row.try_get("name")?,
        method: row.try_get("method")?,
        path: row.try_get("path")?,
        interval_seconds: row.try_get("interval_seconds")?,
        timeout_ms: row.try_get("timeout_ms")?,
        expected_status: row.try_get("expected_status")?,
        enabled: row.try_get::<i32, _>("enabled")? != 0,
        created_at: parse_dt(&row.try_get::<String, _>("created_at")?)?,
        updated_at: parse_dt(&row.try_get::<String, _>("updated_at")?)?,
    })
}

fn row_to_probe_result(row: sqlx::sqlite::SqliteRow) -> Result<ProbeResult, GatewayError> {
    Ok(ProbeResult {
        id: parse_uuid(&row.try_get::<String, _>("id")?)?,
        probe_id: parse_uuid(&row.try_get::<String, _>("probe_id")?)?,
        ts: parse_dt(&row.try_get::<String, _>("ts")?)?,
        status: row.try_get("status")?,
        latency_ms: row.try_get("latency_ms")?,
        healthy: row.try_get::<i32, _>("healthy")? != 0,
        error: row.try_get("error")?,
    })
}

fn row_to_snapshot(row: sqlx::sqlite::SqliteRow) -> Result<Snapshot, GatewayError> {
    let data_str: String = row.try_get("data")?;
    Ok(Snapshot {
        id: parse_uuid(&row.try_get::<String, _>("id")?)?,
        service_id: parse_uuid(&row.try_get::<String, _>("service_id")?)?,
        name: row.try_get("name")?,
        data: serde_json::from_str(&data_str).unwrap_or_default(),
        created_at: parse_dt(&row.try_get::<String, _>("created_at")?)?,
    })
}

fn row_to_alert_rule(row: sqlx::sqlite::SqliteRow) -> Result<AlertRule, GatewayError> {
    let channels_str: String = row.try_get("channels")?;
    Ok(AlertRule {
        id: parse_uuid(&row.try_get::<String, _>("id")?)?,
        service_id: parse_uuid(&row.try_get::<String, _>("service_id")?)?,
        name: row.try_get("name")?,
        metric: row.try_get("metric")?,
        operator: row.try_get("operator")?,
        threshold: row.try_get("threshold")?,
        window_seconds: row.try_get("window_seconds")?,
        cooldown_seconds: row.try_get("cooldown_seconds")?,
        channels: serde_json::from_str(&channels_str).unwrap_or_default(),
        enabled: row.try_get::<i32, _>("enabled")? != 0,
        created_at: parse_dt(&row.try_get::<String, _>("created_at")?)?,
        updated_at: parse_dt(&row.try_get::<String, _>("updated_at")?)?,
    })
}

fn row_to_alert_event(row: sqlx::sqlite::SqliteRow) -> Result<AlertEvent, GatewayError> {
    let resolved_at: Option<String> = row.try_get("resolved_at")?;
    Ok(AlertEvent {
        id: parse_uuid(&row.try_get::<String, _>("id")?)?,
        rule_id: parse_uuid(&row.try_get::<String, _>("rule_id")?)?,
        service_id: parse_uuid(&row.try_get::<String, _>("service_id")?)?,
        ts: parse_dt(&row.try_get::<String, _>("ts")?)?,
        metric: row.try_get("metric")?,
        value: row.try_get("value")?,
        threshold: row.try_get("threshold")?,
        message: row.try_get("message")?,
        resolved: row.try_get::<i32, _>("resolved")? != 0,
        resolved_at: resolved_at.and_then(|s| parse_dt(&s).ok()),
    })
}

fn row_to_ip_rule(row: sqlx::sqlite::SqliteRow) -> Result<IpRule, GatewayError> {
    Ok(IpRule {
        id: parse_uuid(&row.try_get::<String, _>("id")?)?,
        service_id: parse_uuid(&row.try_get::<String, _>("service_id")?)?,
        route_id: row.try_get::<Option<String>, _>("route_id")?.and_then(|s| Uuid::from_str(&s).ok()),
        cidr: row.try_get("cidr")?,
        action: row.try_get("action")?,
        description: row.try_get("description")?,
        priority: row.try_get("priority")?,
        enabled: row.try_get::<i32, _>("enabled")? != 0,
        hits: row.try_get("hits")?,
        created_at: parse_dt(&row.try_get::<String, _>("created_at")?)?,
        updated_at: parse_dt(&row.try_get::<String, _>("updated_at")?)?,
    })
}

fn row_to_error_page(row: sqlx::sqlite::SqliteRow) -> Result<ErrorPage, GatewayError> {
    Ok(ErrorPage {
        id: parse_uuid(&row.try_get::<String, _>("id")?)?,
        service_id: parse_uuid(&row.try_get::<String, _>("service_id")?)?,
        status_code: row.try_get("status_code")?,
        content_type: row.try_get("content_type")?,
        body: row.try_get("body")?,
        enabled: row.try_get::<i32, _>("enabled")? != 0,
        created_at: parse_dt(&row.try_get::<String, _>("created_at")?)?,
        updated_at: parse_dt(&row.try_get::<String, _>("updated_at")?)?,
    })
}

fn row_to_consumer_group(row: sqlx::sqlite::SqliteRow) -> Result<ConsumerGroup, GatewayError> {
    Ok(ConsumerGroup {
        id: parse_uuid(&row.try_get::<String, _>("id")?)?,
        service_id: parse_uuid(&row.try_get::<String, _>("service_id")?)?,
        name: row.try_get("name")?,
        description: row.try_get("description")?,
        rate_limit: row.try_get("rate_limit")?,
        rate_window_seconds: row.try_get("rate_window_seconds")?,
        burst_limit: row.try_get("burst_limit")?,
        created_at: parse_dt(&row.try_get::<String, _>("created_at")?)?,
        updated_at: parse_dt(&row.try_get::<String, _>("updated_at")?)?,
    })
}

fn row_to_consumer_group_member(row: sqlx::sqlite::SqliteRow) -> Result<ConsumerGroupMember, GatewayError> {
    Ok(ConsumerGroupMember {
        group_id: parse_uuid(&row.try_get::<String, _>("group_id")?)?,
        consumer_id: parse_uuid(&row.try_get::<String, _>("consumer_id")?)?,
        joined_at: parse_dt(&row.try_get::<String, _>("joined_at")?)?,
    })
}

#[cfg(test)]
mod recording_tests {
    use super::*;
    use crate::recording::{RecordingFilter, RecordingIndexRow};
    use chrono::{Duration, Utc};

    async fn fresh_repo() -> SqliteRepository {
        // Each test gets its own tempfile DB so migrations run cleanly and
        // tests don't interfere with one another.
        let dir = std::env::temp_dir();
        let path = dir.join(format!("lynxx-test-{}.db", uuid::Uuid::new_v4()));
        let url = format!("sqlite://{}?mode=rwc", path.display());
        SqliteRepository::new(&url).await.unwrap()
    }

    fn sample_row(
        id: &str,
        ts: chrono::DateTime<Utc>,
        method: &str,
        path: &str,
        status: u16,
    ) -> RecordingIndexRow {
        RecordingIndexRow {
            id: id.into(),
            ts,
            method: method.into(),
            path: path.into(),
            status,
            duration_ms: 7,
            service_id: None,
            route_id: None,
            consumer: None,
            bytes_request: 11,
            bytes_response: 22,
        }
    }

    #[tokio::test]
    async fn append_then_get_round_trips() {
        let repo = fresh_repo().await;
        let row = sample_row("aabbccddeeff0011", Utc::now(), "GET", "/v1/x", 200);
        repo.append_recording(&row).await.unwrap();
        let got = repo
            .get_recording_index("aabbccddeeff0011")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(got.id, row.id);
        assert_eq!(got.method, "GET");
        assert_eq!(got.status, 200);
        assert_eq!(got.bytes_request, 11);
        assert_eq!(got.bytes_response, 22);
    }

    #[tokio::test]
    async fn list_filters_method_status_since_limit() {
        let repo = fresh_repo().await;
        let now = Utc::now();
        repo.append_recording(&sample_row(
            "11111111aaaa",
            now - Duration::hours(2),
            "GET",
            "/a",
            200,
        ))
        .await
        .unwrap();
        repo.append_recording(&sample_row(
            "22222222bbbb",
            now - Duration::hours(1),
            "POST",
            "/b",
            500,
        ))
        .await
        .unwrap();
        repo.append_recording(&sample_row("33333333cccc", now, "GET", "/a/sub", 200))
            .await
            .unwrap();

        // method filter
        let got = repo
            .list_recordings(&RecordingFilter {
                method: Some("GET".into()),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(got.len(), 2);
        // newest first
        assert_eq!(got[0].id, "33333333cccc");

        // status filter
        let got = repo
            .list_recordings(&RecordingFilter {
                status: Some(500),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].id, "22222222bbbb");

        // since filter
        let got = repo
            .list_recordings(&RecordingFilter {
                since: Some(now - Duration::minutes(30)),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].id, "33333333cccc");

        // path prefix
        let got = repo
            .list_recordings(&RecordingFilter {
                path_prefix: Some("/a".into()),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(got.len(), 2);

        // limit
        let got = repo
            .list_recordings(&RecordingFilter {
                limit: 1,
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(got.len(), 1);
    }

    #[tokio::test]
    async fn delete_returns_true_then_false() {
        let repo = fresh_repo().await;
        let row = sample_row("ffeeddccbbaa9988", Utc::now(), "GET", "/x", 204);
        repo.append_recording(&row).await.unwrap();
        assert!(repo.delete_recording(&row.id).await.unwrap());
        assert!(!repo.delete_recording(&row.id).await.unwrap());
    }

    #[tokio::test]
    async fn delete_before_with_far_future_clears_all() {
        let repo = fresh_repo().await;
        repo.append_recording(&sample_row("aaaa1111", Utc::now(), "GET", "/a", 200))
            .await
            .unwrap();
        repo.append_recording(&sample_row("bbbb2222", Utc::now(), "GET", "/b", 200))
            .await
            .unwrap();
        let n = repo
            .delete_recordings_before(Utc::now() + Duration::days(365))
            .await
            .unwrap();
        assert_eq!(n, 2);
        let remaining = repo
            .list_recordings(&RecordingFilter::default())
            .await
            .unwrap();
        assert!(remaining.is_empty());
    }
}
