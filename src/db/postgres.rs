use crate::audit::{compute_hash, AuditEntry, AuditFilter, NewAuditEntry};
use crate::auth::{AdminToken, Role};
use crate::cluster::postgres::PostgresBus;
use crate::cluster::ClusterBus;
use crate::config::GatewayConfig;
use crate::db::sqlite::seed_from_config_generic;
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
use sqlx::postgres::{PgPool, PgPoolOptions};
use sqlx::Row;
use std::str::FromStr;
use std::sync::Arc;
use uuid::Uuid;

/// Postgres-backed repository. Used in production / multi-node deployments.
/// Schema parallels the SQLite version but uses native Postgres types
/// (UUID, BOOLEAN, TIMESTAMPTZ, JSONB).
pub struct PostgresRepository {
    pool: PgPool,
}

impl PostgresRepository {
    pub async fn new(database_url: &str) -> Result<Self, GatewayError> {
        let pool = PgPoolOptions::new()
            .max_connections(8)
            .connect(database_url)
            .await?;

        sqlx::migrate!("./migrations/postgres").run(&pool).await?;

        Ok(Self { pool })
    }

    pub fn pool(&self) -> &PgPool {
        &self.pool
    }
}

#[async_trait]
impl Repository for PostgresRepository {
    // ---------- projects ----------

    async fn list_projects(&self) -> Result<Vec<Project>, GatewayError> {
        let rows = sqlx::query(
            "SELECT id, name, description, created_at, updated_at \
             FROM projects WHERE is_deleted = FALSE",
        )
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(row_to_project).collect()
    }

    async fn get_project(&self, id: &Uuid) -> Result<Option<Project>, GatewayError> {
        let row = sqlx::query(
            "SELECT id, name, description, created_at, updated_at \
             FROM projects WHERE id = $1 AND is_deleted = FALSE",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        row.map(row_to_project).transpose()
    }

    async fn create_project(&self, project: &Project) -> Result<(), GatewayError> {
        sqlx::query(
            "INSERT INTO projects (id, name, description, created_at, updated_at) \
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(project.id)
        .bind(&project.name)
        .bind(&project.description)
        .bind(project.created_at)
        .bind(project.updated_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn update_project(&self, id: &Uuid, project: &Project) -> Result<(), GatewayError> {
        sqlx::query(
            "UPDATE projects SET name = $1, description = $2, updated_at = $3 \
             WHERE id = $4 AND is_deleted = FALSE",
        )
        .bind(&project.name)
        .bind(&project.description)
        .bind(project.updated_at)
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn delete_project(&self, id: &Uuid) -> Result<bool, GatewayError> {
        let result = sqlx::query(
            "UPDATE projects SET is_deleted = TRUE, name = name || '__deleted_' || id \
             WHERE id = $1 AND is_deleted = FALSE",
        )
        .bind(id)
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
             FROM services WHERE project_id = $1 AND is_deleted = FALSE",
        )
        .bind(project_id)
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
             FROM services WHERE is_deleted = FALSE",
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
             FROM services WHERE id = $1 AND is_deleted = FALSE",
        )
        .bind(id)
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
             FROM services WHERE name = $1 AND is_deleted = FALSE",
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
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, TRUE, $18, $19)",
        )
        .bind(svc.id)
        .bind(&svc.name)
        .bind(&svc.url)
        .bind(svc.retries as i64)
        .bind(svc.connect_timeout_ms as i64)
        .bind(&svc.lb_strategy)
        .bind(&svc.service_type)
        .bind(svc.document_root.as_deref())
        .bind(&svc.index_file)
        .bind(svc.spa_fallback)
        .bind(svc.tls_verify)
        .bind(svc.tls_client_cert_path.as_deref())
        .bind(svc.tls_client_key_path.as_deref())
        .bind(svc.tls_ca_path.as_deref())
        .bind(svc.project_id)
        .bind(&svc.domain)
        .bind(&svc.subdomain)
        .bind(svc.created_at)
        .bind(svc.updated_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn update_service(&self, id: &Uuid, svc: &Service) -> Result<(), GatewayError> {
        sqlx::query(
            "UPDATE services SET name = $1, url = $2, retries = $3, connect_timeout_ms = $4, lb_strategy = $5, service_type = $6, \
                                 document_root = $7, index_file = $8, spa_fallback = $9, \
                                 tls_verify = $10, tls_client_cert_path = $11, tls_client_key_path = $12, tls_ca_path = $13, \
                                 project_id = $14, domain = $15, subdomain = $16, \
                                 updated_at = $17 \
             WHERE id = $18 AND is_deleted = FALSE",
        )
        .bind(&svc.name)
        .bind(&svc.url)
        .bind(svc.retries as i64)
        .bind(svc.connect_timeout_ms as i64)
        .bind(&svc.lb_strategy)
        .bind(&svc.service_type)
        .bind(svc.document_root.as_deref())
        .bind(&svc.index_file)
        .bind(svc.spa_fallback)
        .bind(svc.tls_verify)
        .bind(svc.tls_client_cert_path.as_deref())
        .bind(svc.tls_client_key_path.as_deref())
        .bind(svc.tls_ca_path.as_deref())
        .bind(svc.project_id)
        .bind(&svc.domain)
        .bind(&svc.subdomain)
        .bind(svc.updated_at)
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn delete_service(&self, id: &Uuid) -> Result<bool, GatewayError> {
        let result = sqlx::query(
            "UPDATE services SET is_deleted = TRUE, name = name || '__deleted_' || id \
             WHERE id = $1 AND is_deleted = FALSE",
        )
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() > 0)
    }

    // ---------- targets ----------

    async fn list_targets(&self) -> Result<Vec<Target>, GatewayError> {
        let rows = sqlx::query(
            "SELECT id, service_id, url, weight, enabled, zone, state, created_at, updated_at FROM targets WHERE is_deleted = FALSE",
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
             FROM targets WHERE service_id = $1 AND is_deleted = FALSE",
        )
        .bind(service_id)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(row_to_target).collect()
    }

    async fn get_target(&self, id: &Uuid) -> Result<Option<Target>, GatewayError> {
        let row = sqlx::query(
            "SELECT id, service_id, url, weight, enabled, zone, state, created_at, updated_at \
             FROM targets WHERE id = $1 AND is_deleted = FALSE",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        row.map(row_to_target).transpose()
    }

    async fn create_target(&self, target: &Target) -> Result<(), GatewayError> {
        sqlx::query(
            "INSERT INTO targets (id, service_id, url, weight, enabled, zone, state, created_at, updated_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
        )
        .bind(target.id)
        .bind(target.service_id)
        .bind(&target.url)
        .bind(target.weight as i64)
        .bind(target.enabled)
        .bind(&target.zone)
        .bind(target.state.as_str())
        .bind(target.created_at)
        .bind(target.updated_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn update_target(&self, id: &Uuid, target: &Target) -> Result<(), GatewayError> {
        sqlx::query(
            "UPDATE targets SET url = $1, weight = $2, enabled = $3, zone = $4, state = $5, updated_at = $6 WHERE id = $7 AND is_deleted = FALSE",
        )
        .bind(&target.url)
        .bind(target.weight as i64)
        .bind(target.enabled)
        .bind(&target.zone)
        .bind(target.state.as_str())
        .bind(target.updated_at)
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn delete_target(&self, id: &Uuid) -> Result<bool, GatewayError> {
        let result = sqlx::query("UPDATE targets SET is_deleted = TRUE WHERE id = $1 AND is_deleted = FALSE")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    async fn update_target_state(&self, id: &Uuid, state: &str) -> Result<bool, GatewayError> {
        let result = sqlx::query(
            "UPDATE targets SET state = $1, updated_at = now() WHERE id = $2 AND is_deleted = FALSE",
        )
        .bind(state)
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() > 0)
    }

    // ---------- routes ----------

    async fn list_routes(&self) -> Result<Vec<Route>, GatewayError> {
        let rows = sqlx::query(
            "SELECT id, service_id, paths, methods, hosts, strip_path, created_at, updated_at \
             FROM routes WHERE is_deleted = FALSE",
        )
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter().map(row_to_route).collect()
    }

    async fn get_route(&self, id: &Uuid) -> Result<Option<Route>, GatewayError> {
        let row = sqlx::query(
            "SELECT id, service_id, paths, methods, hosts, strip_path, created_at, updated_at \
             FROM routes WHERE id = $1 AND is_deleted = FALSE",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;

        row.map(row_to_route).transpose()
    }

    async fn create_route(&self, route: &Route) -> Result<(), GatewayError> {
        let paths = serde_json::to_value(&route.paths)
            .map_err(|e| GatewayError::Internal(e.to_string()))?;
        let methods = serde_json::to_value(&route.methods)
            .map_err(|e| GatewayError::Internal(e.to_string()))?;
        let hosts = serde_json::to_value(&route.hosts)
            .map_err(|e| GatewayError::Internal(e.to_string()))?;

        sqlx::query(
            "INSERT INTO routes (id, service_id, paths, methods, hosts, strip_path, enabled, created_at, updated_at) \
             VALUES ($1, $2, $3, $4, $5, $6, TRUE, $7, $8)",
        )
        .bind(route.id)
        .bind(route.service_id)
        .bind(paths)
        .bind(methods)
        .bind(hosts)
        .bind(route.strip_path)
        .bind(route.created_at)
        .bind(route.updated_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn update_route(&self, id: &Uuid, route: &Route) -> Result<(), GatewayError> {
        let paths = serde_json::to_value(&route.paths)
            .map_err(|e| GatewayError::Internal(e.to_string()))?;
        let methods = serde_json::to_value(&route.methods)
            .map_err(|e| GatewayError::Internal(e.to_string()))?;
        let hosts = serde_json::to_value(&route.hosts)
            .map_err(|e| GatewayError::Internal(e.to_string()))?;

        sqlx::query(
            "UPDATE routes SET service_id = $1, paths = $2, methods = $3, hosts = $4, strip_path = $5, updated_at = $6 \
             WHERE id = $7 AND is_deleted = FALSE",
        )
        .bind(route.service_id)
        .bind(paths)
        .bind(methods)
        .bind(hosts)
        .bind(route.strip_path)
        .bind(route.updated_at)
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn delete_route(&self, id: &Uuid) -> Result<bool, GatewayError> {
        let result = sqlx::query("UPDATE routes SET is_deleted = TRUE WHERE id = $1 AND is_deleted = FALSE")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    // ---------- plugins ----------

    async fn list_plugins(&self) -> Result<Vec<PluginInstance>, GatewayError> {
        let rows = sqlx::query(
            "SELECT id, name, service_id, route_id, enabled, config, created_at, updated_at FROM plugins WHERE is_deleted = FALSE",
        )
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter().map(row_to_plugin).collect()
    }

    async fn get_plugin(&self, id: &Uuid) -> Result<Option<PluginInstance>, GatewayError> {
        let row = sqlx::query(
            "SELECT id, name, service_id, route_id, enabled, config, created_at, updated_at \
             FROM plugins WHERE id = $1 AND is_deleted = FALSE",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        row.map(row_to_plugin).transpose()
    }

    async fn create_plugin(&self, plugin: &PluginInstance) -> Result<(), GatewayError> {
        sqlx::query(
            "INSERT INTO plugins (id, name, service_id, route_id, enabled, config, created_at, updated_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
        )
        .bind(plugin.id)
        .bind(&plugin.name)
        .bind(plugin.service_id)
        .bind(plugin.route_id)
        .bind(plugin.enabled)
        .bind(&plugin.config)
        .bind(plugin.created_at)
        .bind(plugin.updated_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn update_plugin(&self, id: &Uuid, plugin: &PluginInstance) -> Result<(), GatewayError> {
        sqlx::query(
            "UPDATE plugins SET name = $1, service_id = $2, route_id = $3, enabled = $4, config = $5, updated_at = $6 \
             WHERE id = $7 AND is_deleted = FALSE",
        )
        .bind(&plugin.name)
        .bind(plugin.service_id)
        .bind(plugin.route_id)
        .bind(plugin.enabled)
        .bind(&plugin.config)
        .bind(plugin.updated_at)
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn delete_plugin(&self, id: &Uuid) -> Result<bool, GatewayError> {
        let result = sqlx::query("UPDATE plugins SET is_deleted = TRUE WHERE id = $1 AND is_deleted = FALSE")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    // ---------- consumers ----------

    async fn list_consumers(&self) -> Result<Vec<Consumer>, GatewayError> {
        let rows = sqlx::query(
            "SELECT id, username, custom_id, api_key, created_at, updated_at FROM consumers WHERE is_deleted = FALSE",
        )
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter().map(row_to_consumer).collect()
    }

    async fn create_consumer(&self, consumer: &Consumer) -> Result<(), GatewayError> {
        sqlx::query(
            "INSERT INTO consumers (id, username, custom_id, api_key, created_at, updated_at) \
             VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(consumer.id)
        .bind(&consumer.username)
        .bind(consumer.custom_id.as_deref())
        .bind(consumer.api_key.as_deref())
        .bind(consumer.created_at)
        .bind(consumer.updated_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn delete_consumer(&self, id: &Uuid) -> Result<bool, GatewayError> {
        let result = sqlx::query(
            "UPDATE consumers SET is_deleted = TRUE, username = username || '__deleted_' || id \
             WHERE id = $1 AND is_deleted = FALSE",
        )
        .bind(id)
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
            "UPDATE plugins SET is_deleted = TRUE WHERE route_id = $1 AND is_deleted = FALSE",
        )
        .bind(route_id)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected())
    }

    async fn soft_delete_plugins_for_service(
        &self,
        service_id: &Uuid,
    ) -> Result<u64, GatewayError> {
        let result = sqlx::query(
            "UPDATE plugins SET is_deleted = TRUE WHERE service_id = $1 AND is_deleted = FALSE",
        )
        .bind(service_id)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected())
    }

    async fn soft_delete_routes_for_service(
        &self,
        service_id: &Uuid,
    ) -> Result<Vec<Uuid>, GatewayError> {
        let rows: Vec<(Uuid,)> = sqlx::query_as(
            "SELECT id FROM routes WHERE service_id = $1 AND is_deleted = FALSE",
        )
        .bind(service_id)
        .fetch_all(&self.pool)
        .await?;

        let ids: Vec<Uuid> = rows.iter().map(|(id,)| *id).collect();

        sqlx::query(
            "UPDATE routes SET is_deleted = TRUE WHERE service_id = $1 AND is_deleted = FALSE",
        )
        .bind(service_id)
        .execute(&self.pool)
        .await?;

        Ok(ids)
    }

    async fn soft_delete_targets_for_service(
        &self,
        service_id: &Uuid,
    ) -> Result<u64, GatewayError> {
        let result = sqlx::query(
            "UPDATE targets SET is_deleted = TRUE WHERE service_id = $1 AND is_deleted = FALSE",
        )
        .bind(service_id)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected())
    }

    async fn soft_delete_services_for_project(
        &self,
        project_id: &Uuid,
    ) -> Result<Vec<Uuid>, GatewayError> {
        let rows: Vec<(Uuid,)> = sqlx::query_as(
            "SELECT id FROM services WHERE project_id = $1 AND is_deleted = FALSE",
        )
        .bind(project_id)
        .fetch_all(&self.pool)
        .await?;

        let ids: Vec<Uuid> = rows.iter().map(|(id,)| *id).collect();

        sqlx::query(
            "UPDATE services SET is_deleted = TRUE, name = name || '__deleted_' || id \
             WHERE project_id = $1 AND is_deleted = FALSE",
        )
        .bind(project_id)
        .execute(&self.pool)
        .await?;

        Ok(ids)
    }

    // ---------- bootstrap helpers ----------

    async fn is_empty(&self) -> Result<bool, GatewayError> {
        let total: i64 = sqlx::query_scalar(
            "SELECT (SELECT COUNT(*) FROM services WHERE is_deleted = FALSE) \
                  + (SELECT COUNT(*) FROM routes WHERE is_deleted = FALSE) \
                  + (SELECT COUNT(*) FROM plugins WHERE is_deleted = FALSE) \
                  + (SELECT COUNT(*) FROM consumers WHERE is_deleted = FALSE)",
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
            sqlx::query("UPDATE services SET source = 'config' WHERE id = $1")
                .bind(id)
                .execute(&self.pool)
                .await?;
        }
        for id in route_ids {
            sqlx::query("UPDATE routes SET source = 'config' WHERE id = $1")
                .bind(id)
                .execute(&self.pool)
                .await?;
        }
        for id in plugin_ids {
            sqlx::query("UPDATE plugins SET source = 'config' WHERE id = $1")
                .bind(id)
                .execute(&self.pool)
                .await?;
        }
        Ok(())
    }

    fn cluster_bus(&self) -> Arc<dyn ClusterBus> {
        Arc::new(PostgresBus::new(self.pool.clone()))
    }

    // ---------- admin tokens ----------

    async fn list_admin_tokens(&self) -> Result<Vec<AdminToken>, GatewayError> {
        let rows = sqlx::query(
            "SELECT id, name, role, created_at, expires_at, last_used_at \
             FROM admin_tokens WHERE is_deleted = FALSE ORDER BY created_at ASC",
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
             FROM admin_tokens WHERE token_hash = $1 AND is_deleted = FALSE",
        )
        .bind(token_hash)
        .fetch_optional(&self.pool)
        .await?;
        row.map(row_to_admin_token).transpose()
    }

    async fn get_admin_token(&self, id: &Uuid) -> Result<Option<AdminToken>, GatewayError> {
        let row = sqlx::query(
            "SELECT id, name, role, created_at, expires_at, last_used_at \
             FROM admin_tokens WHERE id = $1 AND is_deleted = FALSE",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        row.map(row_to_admin_token).transpose()
    }

    async fn create_admin_token(&self, token: &NewAdminToken) -> Result<(), GatewayError> {
        sqlx::query(
            "INSERT INTO admin_tokens (id, name, token_hash, role, created_at, expires_at) \
             VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(token.id)
        .bind(&token.name)
        .bind(&token.token_hash)
        .bind(token.role.to_string())
        .bind(token.created_at)
        .bind(token.expires_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn update_admin_token(
        &self,
        id: &Uuid,
        patch: &AdminTokenPatch,
    ) -> Result<Option<AdminToken>, GatewayError> {
        let mut tx = self.pool.begin().await?;
        let row = sqlx::query(
            "SELECT id, name, role, created_at, expires_at, last_used_at \
             FROM admin_tokens WHERE id = $1 AND is_deleted = FALSE",
        )
        .bind(id)
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
        sqlx::query("UPDATE admin_tokens SET name = $1, role = $2, expires_at = $3 WHERE id = $4")
            .bind(&existing.name)
            .bind(existing.role.to_string())
            .bind(existing.expires_at)
            .bind(id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(Some(existing))
    }

    async fn delete_admin_token(&self, id: &Uuid) -> Result<bool, GatewayError> {
        let result = sqlx::query(
            "UPDATE admin_tokens SET is_deleted = TRUE, token_hash = token_hash || '__deleted_' || id \
             WHERE id = $1 AND is_deleted = FALSE",
        )
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() > 0)
    }

    async fn update_admin_token_last_used(
        &self,
        id: &Uuid,
        ts: DateTime<Utc>,
    ) -> Result<(), GatewayError> {
        sqlx::query("UPDATE admin_tokens SET last_used_at = $1 WHERE id = $2")
            .bind(ts)
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn has_any_admin_token(&self) -> Result<bool, GatewayError> {
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM admin_tokens WHERE is_deleted = FALSE")
            .fetch_one(&self.pool)
            .await?;
        Ok(count > 0)
    }

    // ---------- audit log ----------

    async fn append_audit(&self, entry: &NewAuditEntry) -> Result<AuditEntry, GatewayError> {
        let mut tx = self.pool.begin().await?;
        let prev_hash: Option<String> =
            sqlx::query_scalar("SELECT hash FROM audit_log ORDER BY id DESC LIMIT 1")
                .fetch_optional(&mut *tx)
                .await?;
        let hash = compute_hash(prev_hash.as_deref(), entry);

        let row = sqlx::query(
            "INSERT INTO audit_log (ts, actor_id, actor_name, actor_role, action, resource_type, resource_id, diff, hash, prev_hash) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10) RETURNING id",
        )
        .bind(entry.ts)
        .bind(entry.actor_id)
        .bind(entry.actor_name.as_deref())
        .bind(entry.actor_role.as_deref())
        .bind(&entry.action)
        .bind(&entry.resource_type)
        .bind(entry.resource_id.as_deref())
        .bind(&entry.diff)
        .bind(&hash)
        .bind(prev_hash.as_deref())
        .fetch_one(&mut *tx)
        .await?;
        let id: i64 = row.try_get("id")?;
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
        let diff_col = if filter.include_diff { "diff" } else { "NULL::jsonb AS diff" };
        let mut sql = format!(
            "SELECT id, ts, actor_id, actor_name, actor_role, action, resource_type, resource_id, {diff_col}, hash, prev_hash FROM audit_log WHERE TRUE",
        );
        let mut idx = 1;
        let mut params: Vec<String> = Vec::new();
        let mut actor_uuid: Option<Uuid> = None;
        if let Some(actor) = &filter.actor {
            actor_uuid = Uuid::parse_str(actor).ok();
            if actor_uuid.is_some() {
                sql.push_str(&format!(
                    " AND (actor_id = ${} OR actor_name = ${})",
                    idx,
                    idx + 1
                ));
                idx += 2;
                params.push(actor.clone());
            } else {
                sql.push_str(&format!(" AND actor_name = ${}", idx));
                idx += 1;
                params.push(actor.clone());
            }
        }
        if let Some(action) = &filter.action {
            sql.push_str(&format!(" AND action = ${}", idx));
            idx += 1;
            params.push(action.clone());
        }
        if let Some(rt) = &filter.resource_type {
            sql.push_str(&format!(" AND resource_type = ${}", idx));
            idx += 1;
            params.push(rt.clone());
        }
        let mut since_param: Option<DateTime<Utc>> = None;
        if let Some(since) = filter.since {
            sql.push_str(&format!(" AND ts >= ${}", idx));
            idx += 1;
            since_param = Some(since);
        }
        sql.push_str(&format!(" ORDER BY ts DESC, id DESC LIMIT ${}", idx));

        let mut q = sqlx::query(&sql);
        if let Some(actor) = &filter.actor {
            if let Some(u) = actor_uuid {
                q = q.bind(u).bind(actor);
            } else {
                q = q.bind(actor);
            }
        }
        // skip the actor entries we already bound; bind remaining text params
        // beyond the actor.
        let actor_params_consumed = if filter.actor.is_some() { 1 } else { 0 };
        for p in params.iter().skip(actor_params_consumed) {
            q = q.bind(p);
        }
        if let Some(s) = since_param {
            q = q.bind(s);
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
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)",
        )
        .bind(&row.id)
        .bind(row.ts)
        .bind(&row.method)
        .bind(&row.path)
        .bind(row.status as i16)
        .bind(row.duration_ms as i64)
        .bind(row.service_id)
        .bind(row.route_id)
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
             FROM recordings WHERE is_deleted = FALSE",
        );
        let mut idx = 1;
        if filter.since.is_some() {
            sql.push_str(&format!(" AND ts >= ${}", idx));
            idx += 1;
        }
        if filter.method.is_some() {
            sql.push_str(&format!(" AND method = ${}", idx));
            idx += 1;
        }
        if filter.path_prefix.is_some() {
            sql.push_str(&format!(" AND path LIKE ${}", idx));
            idx += 1;
        }
        if filter.status.is_some() {
            sql.push_str(&format!(" AND status = ${}", idx));
            idx += 1;
        }
        if filter.service_id.is_some() {
            sql.push_str(&format!(" AND service_id = ${}", idx));
            idx += 1;
        }
        sql.push_str(&format!(" ORDER BY ts DESC LIMIT ${}", idx));

        let mut q = sqlx::query(&sql);
        if let Some(since) = filter.since {
            q = q.bind(since);
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
            q = q.bind(st as i16);
        }
        if let Some(sid) = filter.service_id {
            q = q.bind(sid);
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
             FROM recordings WHERE id = $1 AND is_deleted = FALSE",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        row.map(row_to_recording_index).transpose()
    }

    async fn delete_recording(&self, id: &str) -> Result<bool, GatewayError> {
        let result = sqlx::query("UPDATE recordings SET is_deleted = TRUE WHERE id = $1 AND is_deleted = FALSE")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    async fn delete_recordings_before(&self, ts: DateTime<Utc>) -> Result<u64, GatewayError> {
        let result = sqlx::query("UPDATE recordings SET is_deleted = TRUE WHERE ts < $1 AND is_deleted = FALSE")
            .bind(ts)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected())
    }

    // ---------- commercial zero-trust ----------

    async fn get_device_fingerprint(
        &self,
        customer_id: i64,
        hardware_id: &str,
    ) -> Result<Option<crate::models::DeviceFingerprint>, GatewayError> {
        let row = sqlx::query(
            "SELECT id, customer_id, hardware_id, name, os_info, is_blocked, last_seen_at \
             FROM device_fingerprints WHERE customer_id = $1 AND hardware_id = $2",
        )
        .bind(customer_id)
        .bind(hardware_id)
        .fetch_optional(&self.pool)
        .await?;

        row.map(row_to_device_fingerprint).transpose()
    }

    async fn create_device_fingerprint(
        &self,
        device: &crate::models::DeviceFingerprint,
    ) -> Result<(), GatewayError> {
        sqlx::query(
            "INSERT INTO device_fingerprints (id, customer_id, hardware_id, name, os_info, is_blocked, last_seen_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(device.id)
        .bind(device.customer_id)
        .bind(&device.hardware_id)
        .bind(device.name.as_deref())
        .bind(device.os_info.as_deref())
        .bind(device.is_blocked)
        .bind(device.last_seen_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn update_device_last_seen(
        &self,
        id: &Uuid,
        ts: DateTime<Utc>,
    ) -> Result<(), GatewayError> {
        sqlx::query("UPDATE device_fingerprints SET last_seen_at = $1 WHERE id = $2")
            .bind(ts)
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn create_runtime_grant(
        &self,
        grant: &crate::models::RuntimeGrant,
    ) -> Result<(), GatewayError> {
        sqlx::query(
            "INSERT INTO runtime_grants (id, customer_id, device_id, token_id, expires_at, revoked_at, last_heartbeat_at, created_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
        )
        .bind(grant.id)
        .bind(grant.customer_id)
        .bind(grant.device_id)
        .bind(&grant.token_id)
        .bind(grant.expires_at)
        .bind(grant.revoked_at)
        .bind(grant.last_heartbeat_at)
        .bind(Utc::now())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn get_runtime_grant_by_token_id(
        &self,
        token_id: &str,
    ) -> Result<Option<crate::models::RuntimeGrant>, GatewayError> {
        let row = sqlx::query(
            "SELECT id, customer_id, device_id, token_id, expires_at, revoked_at, last_heartbeat_at \
             FROM runtime_grants WHERE token_id = $1",
        )
        .bind(token_id)
        .fetch_optional(&self.pool)
        .await?;

        row.map(row_to_runtime_grant).transpose()
    }

    async fn update_grant_heartbeat(
        &self,
        token_id: &str,
        ts: DateTime<Utc>,
    ) -> Result<(), GatewayError> {
        sqlx::query("UPDATE runtime_grants SET last_heartbeat_at = $1 WHERE token_id = $2")
            .bind(ts)
            .bind(token_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn revoke_grant(&self, token_id: &str) -> Result<bool, GatewayError> {
        let result = sqlx::query("UPDATE runtime_grants SET revoked_at = $1 WHERE token_id = $2 AND revoked_at IS NULL")
            .bind(Utc::now())
            .bind(token_id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    async fn get_api_key_by_prefix(
        &self,
        prefix: &str,
    ) -> Result<Option<crate::models::ApiKeyRow>, GatewayError> {
        let row = sqlx::query(
            "SELECT id, customer_id, name, key_hash, key_prefix, scopes \
             FROM api_keys WHERE key_prefix = $1 AND is_deleted = FALSE",
        )
        .bind(prefix)
        .fetch_optional(&self.pool)
        .await?;

        row.map(row_to_api_key_row).transpose()
    }

    // ---------- quota ----------

    async fn get_quota_usage(&self, period: &str) -> Result<Option<(u64, u64)>, GatewayError> {
        let row =
            sqlx::query("SELECT calls_total, calls_rejected FROM quota_usage WHERE period = $1")
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
             VALUES ($1, $2, $3, $4) \
             ON CONFLICT (period) DO UPDATE SET \
               calls_total    = EXCLUDED.calls_total, \
               calls_rejected = EXCLUDED.calls_rejected, \
               updated_at     = EXCLUDED.updated_at",
        )
        .bind(period)
        .bind(calls_total as i64)
        .bind(calls_rejected as i64)
        .bind(Utc::now())
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
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16)",
            )
            .bind(e.id)
            .bind(&e.urn)
            .bind(e.ts)
            .bind(&e.level)
            .bind(&e.message)
            .bind(e.service_id)
            .bind(e.route_id)
            .bind(e.project_id)
            .bind(&e.method)
            .bind(&e.path)
            .bind(e.status)
            .bind(e.duration_ms)
            .bind(&e.client_ip)
            .bind(&e.metadata)
            .bind(e.created_at)
            .bind(e.updated_at)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    async fn list_logs(&self, filter: &LogFilter) -> Result<Vec<LogEntry>, GatewayError> {
        let mut sql = String::from(
            "SELECT id, urn, ts, level, message, service_id, route_id, project_id, \
             method, path, status, duration_ms, client_ip, metadata, created_at, updated_at FROM logs WHERE TRUE",
        );
        let mut idx = 0u32;
        let mut bind_service: Option<Uuid> = None;
        let mut bind_route: Option<Uuid> = None;
        let mut bind_project: Option<Uuid> = None;
        let mut bind_level: Option<String> = None;
        let mut bind_search: Option<String> = None;

        if let Some(sid) = filter.service_id {
            idx += 1;
            sql.push_str(&format!(" AND service_id = ${idx}"));
            bind_service = Some(sid);
        }
        if let Some(rid) = filter.route_id {
            idx += 1;
            sql.push_str(&format!(" AND route_id = ${idx}"));
            bind_route = Some(rid);
        }
        if let Some(pid) = filter.project_id {
            idx += 1;
            sql.push_str(&format!(" AND project_id = ${idx}"));
            bind_project = Some(pid);
        }
        if let Some(ref level) = filter.level {
            idx += 1;
            sql.push_str(&format!(" AND level = ${idx}"));
            bind_level = Some(level.clone());
        }
        if let Some(ref search) = filter.search {
            idx += 1;
            sql.push_str(&format!(
                " AND (message ILIKE ${idx} OR path ILIKE ${idx} OR client_ip ILIKE ${idx})"
            ));
            bind_search = Some(format!("%{search}%"));
        }
        let limit = filter.limit.unwrap_or(200).min(1000);
        sql.push_str(&format!(" ORDER BY ts DESC LIMIT {limit}"));

        let mut q = sqlx::query(&sql);
        if let Some(v) = &bind_service {
            q = q.bind(v);
        }
        if let Some(v) = &bind_route {
            q = q.bind(v);
        }
        if let Some(v) = &bind_project {
            q = q.bind(v);
        }
        if let Some(v) = &bind_level {
            q = q.bind(v);
        }
        if let Some(v) = &bind_search {
            q = q.bind(v);
        }
        let rows = q.fetch_all(&self.pool).await?;
        rows.into_iter().map(row_to_log).collect()
    }

    async fn prune_logs(&self, retention_days: u32) -> Result<u64, GatewayError> {
        let result = sqlx::query(&format!(
            "DELETE FROM logs WHERE ts < now() - interval '{retention_days} days'"
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
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)"
        )
        .bind(b.id).bind(b.service_id).bind(b.route_id)
        .bind(b.bucket_ts).bind(&b.granularity)
        .bind(b.total_requests as i32).bind(b.success_count as i32)
        .bind(b.error_4xx as i32).bind(b.error_5xx as i32)
        .bind(b.total_latency_ms).bind(b.min_latency_ms.map(|v| v as i32))
        .bind(b.max_latency_ms.map(|v| v as i32))
        .bind(b.bytes_in).bind(b.bytes_out)
        .execute(&self.pool).await?;
        Ok(())
    }

    async fn list_metric_buckets(&self, service_id: &Uuid, from: DateTime<Utc>, to: DateTime<Utc>, granularity: &str) -> Result<Vec<MetricBucket>, GatewayError> {
        let rows = sqlx::query("SELECT * FROM metric_buckets WHERE service_id = $1 AND bucket_ts >= $2 AND bucket_ts <= $3 AND granularity = $4 ORDER BY bucket_ts")
            .bind(service_id).bind(from).bind(to).bind(granularity)
            .fetch_all(&self.pool).await?;
        rows.into_iter().map(row_to_metric_bucket).collect()
    }

    async fn upsert_geo_bucket(&self, b: &GeoBucket) -> Result<(), GatewayError> {
        sqlx::query("INSERT INTO geo_buckets (id, service_id, bucket_ts, country_code, city, request_count) VALUES ($1, $2, $3, $4, $5, $6)")
            .bind(b.id).bind(b.service_id).bind(b.bucket_ts)
            .bind(&b.country_code).bind(&b.city).bind(b.request_count as i32)
            .execute(&self.pool).await?;
        Ok(())
    }

    async fn list_geo_buckets(&self, service_id: &Uuid, from: DateTime<Utc>, to: DateTime<Utc>) -> Result<Vec<GeoBucket>, GatewayError> {
        let rows = sqlx::query("SELECT * FROM geo_buckets WHERE service_id = $1 AND bucket_ts >= $2 AND bucket_ts <= $3 ORDER BY bucket_ts")
            .bind(service_id).bind(from).bind(to)
            .fetch_all(&self.pool).await?;
        rows.into_iter().map(row_to_geo_bucket).collect()
    }

    async fn upsert_rate_limit_bucket(&self, b: &RateLimitBucket) -> Result<(), GatewayError> {
        sqlx::query("INSERT INTO rate_limit_buckets (id, service_id, consumer_id, bucket_ts, allowed, throttled) VALUES ($1, $2, $3, $4, $5, $6)")
            .bind(b.id).bind(b.service_id).bind(b.consumer_id)
            .bind(b.bucket_ts).bind(b.allowed as i32).bind(b.throttled as i32)
            .execute(&self.pool).await?;
        Ok(())
    }

    async fn list_rate_limit_buckets(&self, service_id: &Uuid, from: DateTime<Utc>, to: DateTime<Utc>) -> Result<Vec<RateLimitBucket>, GatewayError> {
        let rows = sqlx::query("SELECT * FROM rate_limit_buckets WHERE service_id = $1 AND bucket_ts >= $2 AND bucket_ts <= $3 ORDER BY bucket_ts")
            .bind(service_id).bind(from).bind(to)
            .fetch_all(&self.pool).await?;
        rows.into_iter().map(row_to_rate_limit_bucket).collect()
    }

    // ── Probes ──

    async fn list_probes(&self, service_id: &Uuid) -> Result<Vec<Probe>, GatewayError> {
        let rows = sqlx::query("SELECT * FROM probes WHERE service_id = $1 ORDER BY created_at")
            .bind(service_id).fetch_all(&self.pool).await?;
        rows.into_iter().map(row_to_probe).collect()
    }

    async fn get_probe(&self, id: &Uuid) -> Result<Option<Probe>, GatewayError> {
        let row = sqlx::query("SELECT * FROM probes WHERE id = $1")
            .bind(id).fetch_optional(&self.pool).await?;
        row.map(row_to_probe).transpose()
    }

    async fn create_probe(&self, p: &Probe) -> Result<(), GatewayError> {
        sqlx::query("INSERT INTO probes (id, service_id, route_id, name, method, path, interval_seconds, timeout_ms, expected_status, enabled, created_at, updated_at) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12)")
            .bind(p.id).bind(p.service_id).bind(p.route_id)
            .bind(&p.name).bind(&p.method).bind(&p.path)
            .bind(p.interval_seconds).bind(p.timeout_ms).bind(p.expected_status)
            .bind(p.enabled).bind(p.created_at).bind(p.updated_at)
            .execute(&self.pool).await?;
        Ok(())
    }

    async fn update_probe(&self, id: &Uuid, p: &Probe) -> Result<(), GatewayError> {
        sqlx::query("UPDATE probes SET name=$1, method=$2, path=$3, interval_seconds=$4, timeout_ms=$5, expected_status=$6, enabled=$7 WHERE id=$8")
            .bind(&p.name).bind(&p.method).bind(&p.path)
            .bind(p.interval_seconds).bind(p.timeout_ms).bind(p.expected_status)
            .bind(p.enabled).bind(id)
            .execute(&self.pool).await?;
        Ok(())
    }

    async fn delete_probe(&self, id: &Uuid) -> Result<bool, GatewayError> {
        let r = sqlx::query("DELETE FROM probes WHERE id = $1").bind(id).execute(&self.pool).await?;
        Ok(r.rows_affected() > 0)
    }

    async fn append_probe_result(&self, r: &ProbeResult) -> Result<(), GatewayError> {
        sqlx::query("INSERT INTO probe_results (id, probe_id, ts, status, latency_ms, healthy, error) VALUES ($1,$2,$3,$4,$5,$6,$7)")
            .bind(r.id).bind(r.probe_id).bind(r.ts)
            .bind(r.status).bind(r.latency_ms).bind(r.healthy).bind(&r.error)
            .execute(&self.pool).await?;
        Ok(())
    }

    async fn list_probe_results(&self, probe_id: &Uuid, limit: u32) -> Result<Vec<ProbeResult>, GatewayError> {
        let rows = sqlx::query("SELECT * FROM probe_results WHERE probe_id = $1 ORDER BY ts DESC LIMIT $2")
            .bind(probe_id).bind(limit as i64)
            .fetch_all(&self.pool).await?;
        rows.into_iter().map(row_to_probe_result).collect()
    }

    // ── Snapshots ──

    async fn list_snapshots(&self, service_id: &Uuid) -> Result<Vec<Snapshot>, GatewayError> {
        let rows = sqlx::query("SELECT * FROM snapshots WHERE service_id = $1 ORDER BY created_at DESC")
            .bind(service_id).fetch_all(&self.pool).await?;
        rows.into_iter().map(row_to_snapshot).collect()
    }

    async fn get_snapshot(&self, id: &Uuid) -> Result<Option<Snapshot>, GatewayError> {
        let row = sqlx::query("SELECT * FROM snapshots WHERE id = $1")
            .bind(id).fetch_optional(&self.pool).await?;
        row.map(row_to_snapshot).transpose()
    }

    async fn create_snapshot(&self, s: &Snapshot) -> Result<(), GatewayError> {
        sqlx::query("INSERT INTO snapshots (id, service_id, name, data, created_at) VALUES ($1,$2,$3,$4,$5)")
            .bind(s.id).bind(s.service_id).bind(&s.name).bind(&s.data).bind(s.created_at)
            .execute(&self.pool).await?;
        Ok(())
    }

    async fn delete_snapshot(&self, id: &Uuid) -> Result<bool, GatewayError> {
        let r = sqlx::query("DELETE FROM snapshots WHERE id = $1").bind(id).execute(&self.pool).await?;
        Ok(r.rows_affected() > 0)
    }

    // ── Alerts ──

    async fn list_alert_rules(&self, service_id: &Uuid) -> Result<Vec<AlertRule>, GatewayError> {
        let rows = sqlx::query("SELECT * FROM alert_rules WHERE service_id = $1 ORDER BY created_at")
            .bind(service_id).fetch_all(&self.pool).await?;
        rows.into_iter().map(row_to_alert_rule).collect()
    }

    async fn get_alert_rule(&self, id: &Uuid) -> Result<Option<AlertRule>, GatewayError> {
        let row = sqlx::query("SELECT * FROM alert_rules WHERE id = $1")
            .bind(id).fetch_optional(&self.pool).await?;
        row.map(row_to_alert_rule).transpose()
    }

    async fn create_alert_rule(&self, r: &AlertRule) -> Result<(), GatewayError> {
        sqlx::query("INSERT INTO alert_rules (id, service_id, name, metric, operator, threshold, window_seconds, cooldown_seconds, channels, enabled, created_at, updated_at) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12)")
            .bind(r.id).bind(r.service_id).bind(&r.name).bind(&r.metric)
            .bind(&r.operator).bind(r.threshold)
            .bind(r.window_seconds).bind(r.cooldown_seconds)
            .bind(&r.channels).bind(r.enabled)
            .bind(r.created_at).bind(r.updated_at)
            .execute(&self.pool).await?;
        Ok(())
    }

    async fn update_alert_rule(&self, id: &Uuid, r: &AlertRule) -> Result<(), GatewayError> {
        sqlx::query("UPDATE alert_rules SET name=$1, metric=$2, operator=$3, threshold=$4, window_seconds=$5, cooldown_seconds=$6, channels=$7, enabled=$8 WHERE id=$9")
            .bind(&r.name).bind(&r.metric).bind(&r.operator).bind(r.threshold)
            .bind(r.window_seconds).bind(r.cooldown_seconds)
            .bind(&r.channels).bind(r.enabled).bind(id)
            .execute(&self.pool).await?;
        Ok(())
    }

    async fn delete_alert_rule(&self, id: &Uuid) -> Result<bool, GatewayError> {
        let r = sqlx::query("DELETE FROM alert_rules WHERE id = $1").bind(id).execute(&self.pool).await?;
        Ok(r.rows_affected() > 0)
    }

    async fn list_alert_events(&self, service_id: &Uuid, limit: u32) -> Result<Vec<AlertEvent>, GatewayError> {
        let rows = sqlx::query("SELECT * FROM alert_events WHERE service_id = $1 ORDER BY ts DESC LIMIT $2")
            .bind(service_id).bind(limit as i64)
            .fetch_all(&self.pool).await?;
        rows.into_iter().map(row_to_alert_event).collect()
    }

    async fn create_alert_event(&self, e: &AlertEvent) -> Result<(), GatewayError> {
        sqlx::query("INSERT INTO alert_events (id, rule_id, service_id, ts, metric, value, threshold, message, resolved) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9)")
            .bind(e.id).bind(e.rule_id).bind(e.service_id).bind(e.ts)
            .bind(&e.metric).bind(e.value).bind(e.threshold)
            .bind(&e.message).bind(e.resolved)
            .execute(&self.pool).await?;
        Ok(())
    }

    async fn resolve_alert_event(&self, id: &Uuid) -> Result<bool, GatewayError> {
        let r = sqlx::query("UPDATE alert_events SET resolved = TRUE, resolved_at = now() WHERE id = $1 AND resolved = FALSE")
            .bind(id).execute(&self.pool).await?;
        Ok(r.rows_affected() > 0)
    }

    // ── IP Rules ──

    async fn list_ip_rules(&self, service_id: &Uuid) -> Result<Vec<IpRule>, GatewayError> {
        let rows = sqlx::query("SELECT * FROM ip_rules WHERE service_id = $1 ORDER BY priority, created_at")
            .bind(service_id).fetch_all(&self.pool).await?;
        rows.into_iter().map(row_to_ip_rule).collect()
    }

    async fn get_ip_rule(&self, id: &Uuid) -> Result<Option<IpRule>, GatewayError> {
        let row = sqlx::query("SELECT * FROM ip_rules WHERE id = $1")
            .bind(id).fetch_optional(&self.pool).await?;
        row.map(row_to_ip_rule).transpose()
    }

    async fn create_ip_rule(&self, r: &IpRule) -> Result<(), GatewayError> {
        sqlx::query("INSERT INTO ip_rules (id, service_id, route_id, cidr, action, description, priority, enabled, hits, created_at, updated_at) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)")
            .bind(r.id).bind(r.service_id).bind(r.route_id)
            .bind(&r.cidr).bind(&r.action).bind(&r.description)
            .bind(r.priority).bind(r.enabled).bind(r.hits)
            .bind(r.created_at).bind(r.updated_at)
            .execute(&self.pool).await?;
        Ok(())
    }

    async fn update_ip_rule(&self, id: &Uuid, r: &IpRule) -> Result<(), GatewayError> {
        sqlx::query("UPDATE ip_rules SET cidr=$1, action=$2, description=$3, priority=$4, enabled=$5 WHERE id=$6")
            .bind(&r.cidr).bind(&r.action).bind(&r.description)
            .bind(r.priority).bind(r.enabled).bind(id)
            .execute(&self.pool).await?;
        Ok(())
    }

    async fn delete_ip_rule(&self, id: &Uuid) -> Result<bool, GatewayError> {
        let r = sqlx::query("DELETE FROM ip_rules WHERE id = $1").bind(id).execute(&self.pool).await?;
        Ok(r.rows_affected() > 0)
    }

    // ── Error Pages ──

    async fn list_error_pages(&self, service_id: &Uuid) -> Result<Vec<ErrorPage>, GatewayError> {
        let rows = sqlx::query("SELECT * FROM error_pages WHERE service_id = $1 ORDER BY status_code")
            .bind(service_id).fetch_all(&self.pool).await?;
        rows.into_iter().map(row_to_error_page).collect()
    }

    async fn get_error_page(&self, id: &Uuid) -> Result<Option<ErrorPage>, GatewayError> {
        let row = sqlx::query("SELECT * FROM error_pages WHERE id = $1")
            .bind(id).fetch_optional(&self.pool).await?;
        row.map(row_to_error_page).transpose()
    }

    async fn create_error_page(&self, p: &ErrorPage) -> Result<(), GatewayError> {
        sqlx::query("INSERT INTO error_pages (id, service_id, status_code, content_type, body, enabled, created_at, updated_at) VALUES ($1,$2,$3,$4,$5,$6,$7,$8)")
            .bind(p.id).bind(p.service_id).bind(p.status_code)
            .bind(&p.content_type).bind(&p.body).bind(p.enabled)
            .bind(p.created_at).bind(p.updated_at)
            .execute(&self.pool).await?;
        Ok(())
    }

    async fn update_error_page(&self, id: &Uuid, p: &ErrorPage) -> Result<(), GatewayError> {
        sqlx::query("UPDATE error_pages SET status_code=$1, content_type=$2, body=$3, enabled=$4 WHERE id=$5")
            .bind(p.status_code).bind(&p.content_type).bind(&p.body)
            .bind(p.enabled).bind(id)
            .execute(&self.pool).await?;
        Ok(())
    }

    async fn delete_error_page(&self, id: &Uuid) -> Result<bool, GatewayError> {
        let r = sqlx::query("DELETE FROM error_pages WHERE id = $1").bind(id).execute(&self.pool).await?;
        Ok(r.rows_affected() > 0)
    }

    // ── Consumer Groups ──

    async fn list_consumer_groups(&self, service_id: &Uuid) -> Result<Vec<ConsumerGroup>, GatewayError> {
        let rows = sqlx::query("SELECT * FROM consumer_groups WHERE service_id = $1 ORDER BY created_at")
            .bind(service_id).fetch_all(&self.pool).await?;
        rows.into_iter().map(row_to_consumer_group).collect()
    }

    async fn get_consumer_group(&self, id: &Uuid) -> Result<Option<ConsumerGroup>, GatewayError> {
        let row = sqlx::query("SELECT * FROM consumer_groups WHERE id = $1")
            .bind(id).fetch_optional(&self.pool).await?;
        row.map(row_to_consumer_group).transpose()
    }

    async fn create_consumer_group(&self, g: &ConsumerGroup) -> Result<(), GatewayError> {
        sqlx::query("INSERT INTO consumer_groups (id, service_id, name, description, rate_limit, rate_window_seconds, burst_limit, created_at, updated_at) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9)")
            .bind(g.id).bind(g.service_id).bind(&g.name).bind(&g.description)
            .bind(g.rate_limit).bind(g.rate_window_seconds).bind(g.burst_limit)
            .bind(g.created_at).bind(g.updated_at)
            .execute(&self.pool).await?;
        Ok(())
    }

    async fn update_consumer_group(&self, id: &Uuid, g: &ConsumerGroup) -> Result<(), GatewayError> {
        sqlx::query("UPDATE consumer_groups SET name=$1, description=$2, rate_limit=$3, rate_window_seconds=$4, burst_limit=$5 WHERE id=$6")
            .bind(&g.name).bind(&g.description)
            .bind(g.rate_limit).bind(g.rate_window_seconds).bind(g.burst_limit)
            .bind(id)
            .execute(&self.pool).await?;
        Ok(())
    }

    async fn delete_consumer_group(&self, id: &Uuid) -> Result<bool, GatewayError> {
        let r = sqlx::query("DELETE FROM consumer_groups WHERE id = $1").bind(id).execute(&self.pool).await?;
        Ok(r.rows_affected() > 0)
    }

    async fn list_consumer_group_members(&self, group_id: &Uuid) -> Result<Vec<ConsumerGroupMember>, GatewayError> {
        let rows = sqlx::query("SELECT * FROM consumer_group_members WHERE group_id = $1 ORDER BY joined_at")
            .bind(group_id).fetch_all(&self.pool).await?;
        rows.into_iter().map(row_to_consumer_group_member).collect()
    }

    async fn add_consumer_group_member(&self, m: &ConsumerGroupMember) -> Result<(), GatewayError> {
        sqlx::query("INSERT INTO consumer_group_members (group_id, consumer_id, joined_at) VALUES ($1,$2,$3) ON CONFLICT DO NOTHING")
            .bind(m.group_id).bind(m.consumer_id).bind(m.joined_at)
            .execute(&self.pool).await?;
        Ok(())
    }

    async fn remove_consumer_group_member(&self, group_id: &Uuid, consumer_id: &Uuid) -> Result<bool, GatewayError> {
        let r = sqlx::query("DELETE FROM consumer_group_members WHERE group_id = $1 AND consumer_id = $2")
            .bind(group_id).bind(consumer_id)
            .execute(&self.pool).await?;
        Ok(r.rows_affected() > 0)
    }
}

// ---------- row mappers ----------

fn row_to_service(row: sqlx::postgres::PgRow) -> Result<Service, GatewayError> {
    let id: Uuid = row.try_get("id")?;
    let retries: i64 = row.try_get("retries")?;
    let connect_timeout_ms: i64 = row.try_get("connect_timeout_ms")?;
    let created_at: DateTime<Utc> = row.try_get("created_at")?;
    let updated_at: DateTime<Utc> = row.try_get("updated_at")?;

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
    let spa_fallback: bool = row.try_get("spa_fallback").unwrap_or(false);

    let tls_verify: bool = row.try_get("tls_verify").unwrap_or(true);
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
        id,
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
        project_id: row.try_get::<Option<Uuid>, _>("project_id").ok().flatten(),
        domain: row.try_get("domain").unwrap_or_default(),
        subdomain: row.try_get("subdomain").unwrap_or_default(),
        created_at,
        updated_at,
    })
}

fn row_to_project(row: sqlx::postgres::PgRow) -> Result<Project, GatewayError> {
    let id: Uuid = row.try_get("id")?;
    let created_at: DateTime<Utc> = row.try_get("created_at")?;
    let updated_at: DateTime<Utc> = row.try_get("updated_at")?;
    Ok(Project {
        id,
        name: row.try_get("name")?,
        description: row.try_get("description")?,
        created_at,
        updated_at,
    })
}

fn row_to_target(row: sqlx::postgres::PgRow) -> Result<Target, GatewayError> {
    let id: Uuid = row.try_get("id")?;
    let service_id: Uuid = row.try_get("service_id")?;
    let weight: i64 = row.try_get("weight")?;
    let enabled: bool = row.try_get("enabled")?;
    let zone: Option<String> = row.try_get("zone").ok();
    let state_str: String = row.try_get("state").unwrap_or_else(|_| "active".to_string());
    let created_at: DateTime<Utc> = row.try_get("created_at")?;
    let updated_at: DateTime<Utc> = row.try_get("updated_at")?;

    Ok(Target {
        id,
        service_id,
        url: row.try_get("url")?,
        weight: weight as u32,
        enabled,
        zone,
        state: crate::models::TargetLifecycle::from_str(&state_str),
        created_at,
        updated_at,
    })
}

fn row_to_route(row: sqlx::postgres::PgRow) -> Result<Route, GatewayError> {
    let id: Uuid = row.try_get("id")?;
    let service_id: Uuid = row.try_get("service_id")?;
    let paths: serde_json::Value = row.try_get("paths")?;
    let methods: serde_json::Value = row.try_get("methods")?;
    let hosts: serde_json::Value = row
        .try_get("hosts")
        .unwrap_or(serde_json::Value::Array(Vec::new()));
    let strip_path: bool = row.try_get("strip_path")?;
    let created_at: DateTime<Utc> = row.try_get("created_at")?;
    let updated_at: DateTime<Utc> = row.try_get("updated_at")?;

    Ok(Route {
        id,
        service_id,
        paths: serde_json::from_value(paths)
            .map_err(|e| GatewayError::Internal(format!("invalid paths json: {e}")))?,
        methods: serde_json::from_value(methods)
            .map_err(|e| GatewayError::Internal(format!("invalid methods json: {e}")))?,
        hosts: serde_json::from_value(hosts).unwrap_or_default(),
        strip_path,
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
        created_at,
        updated_at,
    })
}

fn row_to_plugin(row: sqlx::postgres::PgRow) -> Result<PluginInstance, GatewayError> {
    let id: Uuid = row.try_get("id")?;
    let service_id: Option<Uuid> = row.try_get("service_id")?;
    let route_id: Option<Uuid> = row.try_get("route_id")?;
    let enabled: bool = row.try_get("enabled")?;
    let config: serde_json::Value = row.try_get("config")?;
    let created_at: DateTime<Utc> = row.try_get("created_at")?;
    let updated_at: DateTime<Utc> = row.try_get("updated_at")?;

    Ok(PluginInstance {
        id,
        name: row.try_get("name")?,
        service_id,
        route_id,
        enabled,
        config,
        created_at,
        updated_at,
    })
}

fn row_to_admin_token(row: sqlx::postgres::PgRow) -> Result<AdminToken, GatewayError> {
    let id: Uuid = row.try_get("id")?;
    let role: String = row.try_get("role")?;
    let created_at: DateTime<Utc> = row.try_get("created_at")?;
    let expires_at: Option<DateTime<Utc>> = row.try_get("expires_at")?;
    let last_used_at: Option<DateTime<Utc>> = row.try_get("last_used_at")?;
    Ok(AdminToken {
        id,
        name: row.try_get("name")?,
        role: Role::from_str(&role)
            .map_err(|e| GatewayError::Internal(format!("invalid role in db: {e}")))?,
        created_at,
        expires_at,
        last_used_at,
    })
}

fn row_to_audit(row: sqlx::postgres::PgRow) -> Result<AuditEntry, GatewayError> {
    let id: i64 = row.try_get("id")?;
    let ts: DateTime<Utc> = row.try_get("ts")?;
    let actor_id: Option<Uuid> = row.try_get("actor_id")?;
    let diff: Option<serde_json::Value> = row.try_get("diff")?;
    Ok(AuditEntry {
        id,
        ts,
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

fn row_to_recording_index(row: sqlx::postgres::PgRow) -> Result<RecordingIndexRow, GatewayError> {
    let id: String = row.try_get("id")?;
    let ts: DateTime<Utc> = row.try_get("ts")?;
    let status: i16 = row.try_get("status")?;
    let duration_ms: i64 = row.try_get("duration_ms")?;
    let service_id: Option<Uuid> = row.try_get("service_id")?;
    let route_id: Option<Uuid> = row.try_get("route_id")?;
    let consumer: Option<String> = row.try_get("consumer")?;
    let bytes_request: i64 = row.try_get("bytes_request")?;
    let bytes_response: i64 = row.try_get("bytes_response")?;

    Ok(RecordingIndexRow {
        id,
        ts,
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

fn row_to_consumer(row: sqlx::postgres::PgRow) -> Result<Consumer, GatewayError> {
    let id: Uuid = row.try_get("id")?;
    let custom_id: Option<String> = row.try_get("custom_id")?;
    let api_key: Option<String> = row.try_get("api_key")?;
    let created_at: DateTime<Utc> = row.try_get("created_at")?;
    let updated_at: DateTime<Utc> = row.try_get("updated_at")?;

    Ok(Consumer {
        id,
        username: row.try_get("username")?,
        custom_id,
        api_key,
        password_hash: None,
        created_at,
        updated_at,
    })
}

fn row_to_device_fingerprint(row: sqlx::postgres::PgRow) -> Result<crate::models::DeviceFingerprint, GatewayError> {
    let id: Uuid = row.try_get("id")?;
    let customer_id: i64 = row.try_get("customer_id")?;
    let hardware_id: String = row.try_get("hardware_id")?;
    let name: Option<String> = row.try_get("name")?;
    let os_info: Option<String> = row.try_get("os_info")?;
    let is_blocked: bool = row.try_get("is_blocked")?;
    let last_seen_at: DateTime<Utc> = row.try_get("last_seen_at")?;

    Ok(crate::models::DeviceFingerprint {
        id,
        customer_id,
        hardware_id,
        name,
        os_info,
        is_blocked,
        last_seen_at,
    })
}

fn row_to_runtime_grant(row: sqlx::postgres::PgRow) -> Result<crate::models::RuntimeGrant, GatewayError> {
    let id: Uuid = row.try_get("id")?;
    let customer_id: i64 = row.try_get("customer_id")?;
    let device_id: Option<Uuid> = row.try_get("device_id")?;
    let token_id: String = row.try_get("token_id")?;
    let expires_at: DateTime<Utc> = row.try_get("expires_at")?;
    let revoked_at: Option<DateTime<Utc>> = row.try_get("revoked_at")?;
    let last_heartbeat_at: DateTime<Utc> = row.try_get("last_heartbeat_at")?;

    Ok(crate::models::RuntimeGrant {
        id,
        customer_id,
        device_id,
        token_id,
        expires_at,
        revoked_at,
        last_heartbeat_at,
    })
}

fn row_to_log(row: sqlx::postgres::PgRow) -> Result<LogEntry, GatewayError> {
    Ok(LogEntry {
        id: row.try_get("id")?,
        urn: row.try_get("urn")?,
        ts: row.try_get("ts")?,
        level: row.try_get("level")?,
        message: row.try_get("message")?,
        service_id: row.try_get("service_id")?,
        route_id: row.try_get("route_id")?,
        project_id: row.try_get("project_id")?,
        method: row.try_get("method")?,
        path: row.try_get("path")?,
        status: row.try_get("status")?,
        duration_ms: row.try_get("duration_ms")?,
        client_ip: row.try_get("client_ip")?,
        metadata: row.try_get("metadata")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

fn row_to_api_key_row(row: sqlx::postgres::PgRow) -> Result<crate::models::ApiKeyRow, GatewayError> {
    let id: i64 = row.try_get("id")?;
    let customer_id: i64 = row.try_get("customer_id")?;
    let name: String = row.try_get("name")?;
    let key_hash: String = row.try_get("key_hash")?;
    let key_prefix: String = row.try_get("key_prefix")?;
    let scopes: Vec<String> = row.try_get("scopes")?;

    Ok(crate::models::ApiKeyRow {
        id,
        customer_id,
        name,
        key_hash,
        key_prefix,
        scopes,
    })
}

fn row_to_metric_bucket(row: sqlx::postgres::PgRow) -> Result<MetricBucket, GatewayError> {
    Ok(MetricBucket {
        id: row.try_get("id")?,
        service_id: row.try_get("service_id")?,
        route_id: row.try_get("route_id")?,
        bucket_ts: row.try_get("bucket_ts")?,
        granularity: row.try_get("granularity")?,
        total_requests: row.try_get::<i32, _>("total_requests")? as i64,
        success_count: row.try_get::<i32, _>("success_count")? as i64,
        error_4xx: row.try_get::<i32, _>("error_4xx")? as i64,
        error_5xx: row.try_get::<i32, _>("error_5xx")? as i64,
        total_latency_ms: row.try_get("total_latency_ms")?,
        min_latency_ms: row.try_get::<Option<i32>, _>("min_latency_ms")?.map(|v| v as i64),
        max_latency_ms: row.try_get::<Option<i32>, _>("max_latency_ms")?.map(|v| v as i64),
        bytes_in: row.try_get("bytes_in")?,
        bytes_out: row.try_get("bytes_out")?,
    })
}

fn row_to_geo_bucket(row: sqlx::postgres::PgRow) -> Result<GeoBucket, GatewayError> {
    Ok(GeoBucket {
        id: row.try_get("id")?,
        service_id: row.try_get("service_id")?,
        bucket_ts: row.try_get("bucket_ts")?,
        country_code: row.try_get("country_code")?,
        city: row.try_get("city")?,
        request_count: row.try_get::<i32, _>("request_count")? as i64,
    })
}

fn row_to_rate_limit_bucket(row: sqlx::postgres::PgRow) -> Result<RateLimitBucket, GatewayError> {
    Ok(RateLimitBucket {
        id: row.try_get("id")?,
        service_id: row.try_get("service_id")?,
        consumer_id: row.try_get("consumer_id")?,
        bucket_ts: row.try_get("bucket_ts")?,
        allowed: row.try_get::<i32, _>("allowed")? as i64,
        throttled: row.try_get::<i32, _>("throttled")? as i64,
    })
}

fn row_to_probe(row: sqlx::postgres::PgRow) -> Result<Probe, GatewayError> {
    Ok(Probe {
        id: row.try_get("id")?,
        service_id: row.try_get("service_id")?,
        route_id: row.try_get("route_id")?,
        name: row.try_get("name")?,
        method: row.try_get("method")?,
        path: row.try_get("path")?,
        interval_seconds: row.try_get("interval_seconds")?,
        timeout_ms: row.try_get("timeout_ms")?,
        expected_status: row.try_get("expected_status")?,
        enabled: row.try_get("enabled")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

fn row_to_probe_result(row: sqlx::postgres::PgRow) -> Result<ProbeResult, GatewayError> {
    Ok(ProbeResult {
        id: row.try_get("id")?,
        probe_id: row.try_get("probe_id")?,
        ts: row.try_get("ts")?,
        status: row.try_get("status")?,
        latency_ms: row.try_get("latency_ms")?,
        healthy: row.try_get("healthy")?,
        error: row.try_get("error")?,
    })
}

fn row_to_snapshot(row: sqlx::postgres::PgRow) -> Result<Snapshot, GatewayError> {
    Ok(Snapshot {
        id: row.try_get("id")?,
        service_id: row.try_get("service_id")?,
        name: row.try_get("name")?,
        data: row.try_get("data")?,
        created_at: row.try_get("created_at")?,
    })
}

fn row_to_alert_rule(row: sqlx::postgres::PgRow) -> Result<AlertRule, GatewayError> {
    Ok(AlertRule {
        id: row.try_get("id")?,
        service_id: row.try_get("service_id")?,
        name: row.try_get("name")?,
        metric: row.try_get("metric")?,
        operator: row.try_get("operator")?,
        threshold: row.try_get("threshold")?,
        window_seconds: row.try_get("window_seconds")?,
        cooldown_seconds: row.try_get("cooldown_seconds")?,
        channels: row.try_get("channels")?,
        enabled: row.try_get("enabled")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

fn row_to_alert_event(row: sqlx::postgres::PgRow) -> Result<AlertEvent, GatewayError> {
    Ok(AlertEvent {
        id: row.try_get("id")?,
        rule_id: row.try_get("rule_id")?,
        service_id: row.try_get("service_id")?,
        ts: row.try_get("ts")?,
        metric: row.try_get("metric")?,
        value: row.try_get("value")?,
        threshold: row.try_get("threshold")?,
        message: row.try_get("message")?,
        resolved: row.try_get("resolved")?,
        resolved_at: row.try_get("resolved_at")?,
    })
}

fn row_to_ip_rule(row: sqlx::postgres::PgRow) -> Result<IpRule, GatewayError> {
    Ok(IpRule {
        id: row.try_get("id")?,
        service_id: row.try_get("service_id")?,
        route_id: row.try_get("route_id")?,
        cidr: row.try_get("cidr")?,
        action: row.try_get("action")?,
        description: row.try_get("description")?,
        priority: row.try_get("priority")?,
        enabled: row.try_get("enabled")?,
        hits: row.try_get("hits")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

fn row_to_error_page(row: sqlx::postgres::PgRow) -> Result<ErrorPage, GatewayError> {
    Ok(ErrorPage {
        id: row.try_get("id")?,
        service_id: row.try_get("service_id")?,
        status_code: row.try_get("status_code")?,
        content_type: row.try_get("content_type")?,
        body: row.try_get("body")?,
        enabled: row.try_get("enabled")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

fn row_to_consumer_group(row: sqlx::postgres::PgRow) -> Result<ConsumerGroup, GatewayError> {
    Ok(ConsumerGroup {
        id: row.try_get("id")?,
        service_id: row.try_get("service_id")?,
        name: row.try_get("name")?,
        description: row.try_get("description")?,
        rate_limit: row.try_get("rate_limit")?,
        rate_window_seconds: row.try_get("rate_window_seconds")?,
        burst_limit: row.try_get("burst_limit")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

fn row_to_consumer_group_member(row: sqlx::postgres::PgRow) -> Result<ConsumerGroupMember, GatewayError> {
    Ok(ConsumerGroupMember {
        group_id: row.try_get("group_id")?,
        consumer_id: row.try_get("consumer_id")?,
        joined_at: row.try_get("joined_at")?,
    })
}
