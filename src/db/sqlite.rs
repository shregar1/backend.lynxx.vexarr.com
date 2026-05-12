use crate::audit::{compute_hash, AuditEntry, AuditFilter, NewAuditEntry};
use crate::auth::{AdminToken, Role};
use crate::cluster::sqlite::SqliteBus;
use crate::cluster::ClusterBus;
use crate::config::GatewayConfig;
use crate::db::{AdminTokenPatch, NewAdminToken, Repository};
use crate::error::GatewayError;
use crate::models::{Consumer, PluginInstance, Route, Service, Target};
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
    // ---------- services ----------

    async fn list_services(&self) -> Result<Vec<Service>, GatewayError> {
        let rows = sqlx::query(
            "SELECT id, name, url, retries, connect_timeout_ms, lb_strategy, service_type, \
                    document_root, index_file, spa_fallback, \
                    tls_verify, tls_client_cert_path, tls_client_key_path, tls_ca_path, \
                    created_at, updated_at \
             FROM services",
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
                    created_at, updated_at \
             FROM services WHERE id = ?",
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
                    created_at, updated_at \
             FROM services WHERE name = ?",
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
                                   enabled, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 1, ?, ?)",
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
                                 updated_at = ? \
             WHERE id = ?",
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
        .bind(svc.updated_at.to_rfc3339())
        .bind(id.to_string())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn delete_service(&self, id: &Uuid) -> Result<bool, GatewayError> {
        let result = sqlx::query("DELETE FROM services WHERE id = ?")
            .bind(id.to_string())
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    // ---------- targets ----------

    async fn list_targets(&self) -> Result<Vec<Target>, GatewayError> {
        let rows = sqlx::query(
            "SELECT id, service_id, url, weight, enabled, zone, created_at, updated_at FROM targets",
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
            "SELECT id, service_id, url, weight, enabled, zone, created_at, updated_at \
             FROM targets WHERE service_id = ?",
        )
        .bind(service_id.to_string())
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(row_to_target).collect()
    }

    async fn get_target(&self, id: &Uuid) -> Result<Option<Target>, GatewayError> {
        let row = sqlx::query(
            "SELECT id, service_id, url, weight, enabled, zone, created_at, updated_at \
             FROM targets WHERE id = ?",
        )
        .bind(id.to_string())
        .fetch_optional(&self.pool)
        .await?;
        row.map(row_to_target).transpose()
    }

    async fn create_target(&self, target: &Target) -> Result<(), GatewayError> {
        sqlx::query(
            "INSERT INTO targets (id, service_id, url, weight, enabled, zone, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(target.id.to_string())
        .bind(target.service_id.to_string())
        .bind(&target.url)
        .bind(target.weight as i64)
        .bind(target.enabled as i64)
        .bind(target.zone.as_deref())
        .bind(target.created_at.to_rfc3339())
        .bind(target.updated_at.to_rfc3339())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn update_target(&self, id: &Uuid, target: &Target) -> Result<(), GatewayError> {
        sqlx::query(
            "UPDATE targets SET url = ?, weight = ?, enabled = ?, zone = ?, updated_at = ? WHERE id = ?",
        )
        .bind(&target.url)
        .bind(target.weight as i64)
        .bind(target.enabled as i64)
        .bind(target.zone.as_deref())
        .bind(target.updated_at.to_rfc3339())
        .bind(id.to_string())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn delete_target(&self, id: &Uuid) -> Result<bool, GatewayError> {
        let result = sqlx::query("DELETE FROM targets WHERE id = ?")
            .bind(id.to_string())
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    // ---------- routes ----------

    async fn list_routes(&self) -> Result<Vec<Route>, GatewayError> {
        let rows = sqlx::query(
            "SELECT id, service_id, paths, methods, hosts, strip_path, created_at, updated_at \
             FROM routes",
        )
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter().map(row_to_route).collect()
    }

    async fn get_route(&self, id: &Uuid) -> Result<Option<Route>, GatewayError> {
        let row = sqlx::query(
            "SELECT id, service_id, paths, methods, hosts, strip_path, created_at, updated_at \
             FROM routes WHERE id = ?",
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
             WHERE id = ?",
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
        let result = sqlx::query("DELETE FROM routes WHERE id = ?")
            .bind(id.to_string())
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    // ---------- plugins ----------

    async fn list_plugins(&self) -> Result<Vec<PluginInstance>, GatewayError> {
        let rows = sqlx::query(
            "SELECT id, name, service_id, route_id, enabled, config, created_at, updated_at FROM plugins",
        )
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter().map(row_to_plugin).collect()
    }

    async fn get_plugin(&self, id: &Uuid) -> Result<Option<PluginInstance>, GatewayError> {
        let row = sqlx::query(
            "SELECT id, name, service_id, route_id, enabled, config, created_at, updated_at \
             FROM plugins WHERE id = ?",
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
             WHERE id = ?",
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
        let result = sqlx::query("DELETE FROM plugins WHERE id = ?")
            .bind(id.to_string())
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    // ---------- consumers ----------

    async fn list_consumers(&self) -> Result<Vec<Consumer>, GatewayError> {
        let rows = sqlx::query(
            "SELECT id, username, custom_id, api_key, created_at, updated_at FROM consumers",
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
        let result = sqlx::query("DELETE FROM consumers WHERE id = ?")
            .bind(id.to_string())
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    // ---------- bootstrap helpers ----------

    async fn is_empty(&self) -> Result<bool, GatewayError> {
        let total: i64 = sqlx::query_scalar(
            "SELECT (SELECT COUNT(*) FROM services) \
                  + (SELECT COUNT(*) FROM routes) \
                  + (SELECT COUNT(*) FROM plugins) \
                  + (SELECT COUNT(*) FROM consumers)",
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
             FROM admin_tokens ORDER BY created_at ASC",
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
             FROM admin_tokens WHERE token_hash = ?",
        )
        .bind(token_hash)
        .fetch_optional(&self.pool)
        .await?;
        row.map(row_to_admin_token).transpose()
    }

    async fn get_admin_token(&self, id: &Uuid) -> Result<Option<AdminToken>, GatewayError> {
        let row = sqlx::query(
            "SELECT id, name, role, created_at, expires_at, last_used_at \
             FROM admin_tokens WHERE id = ?",
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
             FROM admin_tokens WHERE id = ?",
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
        let result = sqlx::query("DELETE FROM admin_tokens WHERE id = ?")
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
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM admin_tokens")
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
             FROM recordings WHERE 1=1",
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
             FROM recordings WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        row.map(row_to_recording_index).transpose()
    }

    async fn delete_recording(&self, id: &str) -> Result<bool, GatewayError> {
        let result = sqlx::query("DELETE FROM recordings WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    async fn delete_recordings_before(&self, ts: DateTime<Utc>) -> Result<u64, GatewayError> {
        let result = sqlx::query("DELETE FROM recordings WHERE ts < ?")
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
    let created_at: String = row.try_get("created_at")?;
    let updated_at: String = row.try_get("updated_at")?;

    Ok(Target {
        id: parse_uuid(&id)?,
        service_id: parse_uuid(&service_id)?,
        url: row.try_get("url")?,
        weight: weight as u32,
        enabled: enabled != 0,
        zone,
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
