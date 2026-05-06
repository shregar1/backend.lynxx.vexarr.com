use crate::audit::{compute_hash, AuditEntry, AuditFilter, NewAuditEntry};
use crate::auth::{AdminToken, Role};
use crate::cluster::postgres::PostgresBus;
use crate::cluster::ClusterBus;
use crate::config::GatewayConfig;
use crate::db::sqlite::seed_from_config_generic;
use crate::db::{AdminTokenPatch, NewAdminToken, Repository};
use crate::error::GatewayError;
use crate::models::{Consumer, PluginInstance, Route, Service, Target};
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
    // ---------- services ----------

    async fn list_services(&self) -> Result<Vec<Service>, GatewayError> {
        let rows = sqlx::query(
            "SELECT id, name, url, retries, connect_timeout_ms, lb_strategy, \
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
            "SELECT id, name, url, retries, connect_timeout_ms, lb_strategy, \
                    tls_verify, tls_client_cert_path, tls_client_key_path, tls_ca_path, \
                    created_at, updated_at \
             FROM services WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;

        row.map(row_to_service).transpose()
    }

    async fn create_service(&self, svc: &Service) -> Result<(), GatewayError> {
        sqlx::query(
            "INSERT INTO services (id, name, url, retries, connect_timeout_ms, lb_strategy, \
                                   tls_verify, tls_client_cert_path, tls_client_key_path, tls_ca_path, \
                                   enabled, created_at, updated_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, TRUE, $11, $12)",
        )
        .bind(svc.id)
        .bind(&svc.name)
        .bind(&svc.url)
        .bind(svc.retries as i64)
        .bind(svc.connect_timeout_ms as i64)
        .bind(&svc.lb_strategy)
        .bind(svc.tls_verify)
        .bind(svc.tls_client_cert_path.as_deref())
        .bind(svc.tls_client_key_path.as_deref())
        .bind(svc.tls_ca_path.as_deref())
        .bind(svc.created_at)
        .bind(svc.updated_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn update_service(&self, id: &Uuid, svc: &Service) -> Result<(), GatewayError> {
        sqlx::query(
            "UPDATE services SET name = $1, url = $2, retries = $3, connect_timeout_ms = $4, lb_strategy = $5, \
                                 tls_verify = $6, tls_client_cert_path = $7, tls_client_key_path = $8, tls_ca_path = $9, \
                                 updated_at = $10 \
             WHERE id = $11",
        )
        .bind(&svc.name)
        .bind(&svc.url)
        .bind(svc.retries as i64)
        .bind(svc.connect_timeout_ms as i64)
        .bind(&svc.lb_strategy)
        .bind(svc.tls_verify)
        .bind(svc.tls_client_cert_path.as_deref())
        .bind(svc.tls_client_key_path.as_deref())
        .bind(svc.tls_ca_path.as_deref())
        .bind(svc.updated_at)
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn delete_service(&self, id: &Uuid) -> Result<bool, GatewayError> {
        let result = sqlx::query("DELETE FROM services WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    // ---------- targets ----------

    async fn list_targets(&self) -> Result<Vec<Target>, GatewayError> {
        let rows = sqlx::query(
            "SELECT id, service_id, url, weight, enabled, created_at, updated_at FROM targets",
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
            "SELECT id, service_id, url, weight, enabled, created_at, updated_at \
             FROM targets WHERE service_id = $1",
        )
        .bind(service_id)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(row_to_target).collect()
    }

    async fn get_target(&self, id: &Uuid) -> Result<Option<Target>, GatewayError> {
        let row = sqlx::query(
            "SELECT id, service_id, url, weight, enabled, created_at, updated_at \
             FROM targets WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        row.map(row_to_target).transpose()
    }

    async fn create_target(&self, target: &Target) -> Result<(), GatewayError> {
        sqlx::query(
            "INSERT INTO targets (id, service_id, url, weight, enabled, created_at, updated_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(target.id)
        .bind(target.service_id)
        .bind(&target.url)
        .bind(target.weight as i64)
        .bind(target.enabled)
        .bind(target.created_at)
        .bind(target.updated_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn update_target(&self, id: &Uuid, target: &Target) -> Result<(), GatewayError> {
        sqlx::query(
            "UPDATE targets SET url = $1, weight = $2, enabled = $3, updated_at = $4 WHERE id = $5",
        )
        .bind(&target.url)
        .bind(target.weight as i64)
        .bind(target.enabled)
        .bind(target.updated_at)
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn delete_target(&self, id: &Uuid) -> Result<bool, GatewayError> {
        let result = sqlx::query("DELETE FROM targets WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    // ---------- routes ----------

    async fn list_routes(&self) -> Result<Vec<Route>, GatewayError> {
        let rows = sqlx::query(
            "SELECT id, service_id, paths, methods, strip_path, created_at, updated_at \
             FROM routes",
        )
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter().map(row_to_route).collect()
    }

    async fn get_route(&self, id: &Uuid) -> Result<Option<Route>, GatewayError> {
        let row = sqlx::query(
            "SELECT id, service_id, paths, methods, strip_path, created_at, updated_at \
             FROM routes WHERE id = $1",
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
        let hosts = serde_json::Value::Array(Vec::new());

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

        sqlx::query(
            "UPDATE routes SET service_id = $1, paths = $2, methods = $3, strip_path = $4, updated_at = $5 \
             WHERE id = $6",
        )
        .bind(route.service_id)
        .bind(paths)
        .bind(methods)
        .bind(route.strip_path)
        .bind(route.updated_at)
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn delete_route(&self, id: &Uuid) -> Result<bool, GatewayError> {
        let result = sqlx::query("DELETE FROM routes WHERE id = $1")
            .bind(id)
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
             FROM plugins WHERE id = $1",
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
             WHERE id = $7",
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
        let result = sqlx::query("DELETE FROM plugins WHERE id = $1")
            .bind(id)
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
        let result = sqlx::query("DELETE FROM consumers WHERE id = $1")
            .bind(id)
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

    fn cluster_bus(&self) -> Arc<dyn ClusterBus> {
        Arc::new(PostgresBus::new(self.pool.clone()))
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
             FROM admin_tokens WHERE token_hash = $1",
        )
        .bind(token_hash)
        .fetch_optional(&self.pool)
        .await?;
        row.map(row_to_admin_token).transpose()
    }

    async fn get_admin_token(&self, id: &Uuid) -> Result<Option<AdminToken>, GatewayError> {
        let row = sqlx::query(
            "SELECT id, name, role, created_at, expires_at, last_used_at \
             FROM admin_tokens WHERE id = $1",
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
             FROM admin_tokens WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?;
        let Some(row) = row else { return Ok(None); };
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
        sqlx::query(
            "UPDATE admin_tokens SET name = $1, role = $2, expires_at = $3 WHERE id = $4",
        )
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
        let result = sqlx::query("DELETE FROM admin_tokens WHERE id = $1")
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
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM admin_tokens")
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
        let mut sql = String::from(
            "SELECT id, ts, actor_id, actor_name, actor_role, action, resource_type, resource_id, diff, hash, prev_hash FROM audit_log WHERE TRUE",
        );
        let mut idx = 1;
        let mut params: Vec<String> = Vec::new();
        let mut actor_uuid: Option<Uuid> = None;
        if let Some(actor) = &filter.actor {
            actor_uuid = Uuid::parse_str(actor).ok();
            if actor_uuid.is_some() {
                sql.push_str(&format!(" AND (actor_id = ${} OR actor_name = ${})", idx, idx + 1));
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
        sql.push_str(&format!(" ORDER BY id DESC LIMIT ${}", idx));

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
             FROM recordings WHERE TRUE",
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
             FROM recordings WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        row.map(row_to_recording_index).transpose()
    }

    async fn delete_recording(&self, id: &str) -> Result<bool, GatewayError> {
        let result = sqlx::query("DELETE FROM recordings WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    async fn delete_recordings_before(
        &self,
        ts: DateTime<Utc>,
    ) -> Result<u64, GatewayError> {
        let result = sqlx::query("DELETE FROM recordings WHERE ts < $1")
            .bind(ts)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected())
    }

    // ---------- quota ----------

    async fn get_quota_usage(&self, period: &str) -> Result<Option<(u64, u64)>, GatewayError> {
        let row = sqlx::query(
            "SELECT calls_total, calls_rejected FROM quota_usage WHERE period = $1",
        )
        .bind(period)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(|r| {
            let calls: i64    = r.get(0);
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

    let tls_verify: bool = row.try_get("tls_verify").unwrap_or(true);
    let tls_client_cert_path: Option<String> =
        row.try_get::<Option<String>, _>("tls_client_cert_path")
            .ok()
            .flatten();
    let tls_client_key_path: Option<String> =
        row.try_get::<Option<String>, _>("tls_client_key_path")
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
        tls_verify,
        tls_client_cert_path,
        tls_client_key_path,
        tls_ca_path,
        created_at,
        updated_at,
    })
}

fn row_to_target(row: sqlx::postgres::PgRow) -> Result<Target, GatewayError> {
    let id: Uuid = row.try_get("id")?;
    let service_id: Uuid = row.try_get("service_id")?;
    let weight: i64 = row.try_get("weight")?;
    let enabled: bool = row.try_get("enabled")?;
    let created_at: DateTime<Utc> = row.try_get("created_at")?;
    let updated_at: DateTime<Utc> = row.try_get("updated_at")?;

    Ok(Target {
        id,
        service_id,
        url: row.try_get("url")?,
        weight: weight as u32,
        enabled,
        created_at,
        updated_at,
    })
}

fn row_to_route(row: sqlx::postgres::PgRow) -> Result<Route, GatewayError> {
    let id: Uuid = row.try_get("id")?;
    let service_id: Uuid = row.try_get("service_id")?;
    let paths: serde_json::Value = row.try_get("paths")?;
    let methods: serde_json::Value = row.try_get("methods")?;
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
        strip_path,
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

fn row_to_recording_index(
    row: sqlx::postgres::PgRow,
) -> Result<RecordingIndexRow, GatewayError> {
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
        created_at,
        updated_at,
    })
}
