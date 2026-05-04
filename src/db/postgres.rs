use crate::cluster::postgres::PostgresBus;
use crate::cluster::ClusterBus;
use crate::config::GatewayConfig;
use crate::db::sqlite::seed_from_config_generic;
use crate::db::Repository;
use crate::error::GatewayError;
use crate::models::{Consumer, PluginInstance, Route, Service, Target};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::postgres::{PgPool, PgPoolOptions};
use sqlx::Row;
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
            "SELECT id, name, url, retries, connect_timeout_ms, lb_strategy, created_at, updated_at \
             FROM services",
        )
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter().map(row_to_service).collect()
    }

    async fn get_service(&self, id: &Uuid) -> Result<Option<Service>, GatewayError> {
        let row = sqlx::query(
            "SELECT id, name, url, retries, connect_timeout_ms, lb_strategy, created_at, updated_at \
             FROM services WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;

        row.map(row_to_service).transpose()
    }

    async fn create_service(&self, svc: &Service) -> Result<(), GatewayError> {
        sqlx::query(
            "INSERT INTO services (id, name, url, retries, connect_timeout_ms, lb_strategy, enabled, created_at, updated_at) \
             VALUES ($1, $2, $3, $4, $5, $6, TRUE, $7, $8)",
        )
        .bind(svc.id)
        .bind(&svc.name)
        .bind(&svc.url)
        .bind(svc.retries as i64)
        .bind(svc.connect_timeout_ms as i64)
        .bind(&svc.lb_strategy)
        .bind(svc.created_at)
        .bind(svc.updated_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn update_service(&self, id: &Uuid, svc: &Service) -> Result<(), GatewayError> {
        sqlx::query(
            "UPDATE services SET name = $1, url = $2, retries = $3, connect_timeout_ms = $4, lb_strategy = $5, updated_at = $6 \
             WHERE id = $7",
        )
        .bind(&svc.name)
        .bind(&svc.url)
        .bind(svc.retries as i64)
        .bind(svc.connect_timeout_ms as i64)
        .bind(&svc.lb_strategy)
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

    async fn update_plugin(
        &self,
        id: &Uuid,
        plugin: &PluginInstance,
    ) -> Result<(), GatewayError> {
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

    Ok(Service {
        id,
        name: row.try_get("name")?,
        url: row.try_get("url")?,
        retries: retries as u32,
        connect_timeout_ms: connect_timeout_ms as u64,
        lb_strategy,
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
