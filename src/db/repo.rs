use crate::config::GatewayConfig;
use crate::error::GatewayError;
use crate::models::{Consumer, PluginInstance, Route, Service, Target};
use chrono::{DateTime, Utc};
use sqlx::sqlite::{SqlitePool, SqlitePoolOptions};
use sqlx::Row;
use std::str::FromStr;
use uuid::Uuid;

/// SQLite-backed repository. Source of truth for services, routes, plugins,
/// and consumers. The hot-path `RouteTable` is rebuilt from this on startup.
pub struct Repository {
    pool: SqlitePool,
}

impl Repository {
    pub async fn new(database_url: &str) -> Result<Self, GatewayError> {
        let pool = SqlitePoolOptions::new()
            .max_connections(8)
            .connect(database_url)
            .await?;

        sqlx::migrate!("./migrations").run(&pool).await?;

        Ok(Self { pool })
    }

    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    // ---------- services ----------

    pub async fn list_services(&self) -> Result<Vec<Service>, GatewayError> {
        let rows = sqlx::query(
            "SELECT id, name, url, retries, connect_timeout_ms, lb_strategy, created_at, updated_at \
             FROM services",
        )
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter().map(row_to_service).collect()
    }

    pub async fn get_service(&self, id: &Uuid) -> Result<Option<Service>, GatewayError> {
        let row = sqlx::query(
            "SELECT id, name, url, retries, connect_timeout_ms, lb_strategy, created_at, updated_at \
             FROM services WHERE id = ?",
        )
        .bind(id.to_string())
        .fetch_optional(&self.pool)
        .await?;

        row.map(row_to_service).transpose()
    }

    pub async fn create_service(&self, svc: &Service) -> Result<(), GatewayError> {
        sqlx::query(
            "INSERT INTO services (id, name, url, retries, connect_timeout_ms, lb_strategy, enabled, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, 1, ?, ?)",
        )
        .bind(svc.id.to_string())
        .bind(&svc.name)
        .bind(&svc.url)
        .bind(svc.retries as i64)
        .bind(svc.connect_timeout_ms as i64)
        .bind(&svc.lb_strategy)
        .bind(svc.created_at.to_rfc3339())
        .bind(svc.updated_at.to_rfc3339())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn update_service(&self, id: &Uuid, svc: &Service) -> Result<(), GatewayError> {
        sqlx::query(
            "UPDATE services SET name = ?, url = ?, retries = ?, connect_timeout_ms = ?, lb_strategy = ?, updated_at = ? \
             WHERE id = ?",
        )
        .bind(&svc.name)
        .bind(&svc.url)
        .bind(svc.retries as i64)
        .bind(svc.connect_timeout_ms as i64)
        .bind(&svc.lb_strategy)
        .bind(svc.updated_at.to_rfc3339())
        .bind(id.to_string())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn delete_service(&self, id: &Uuid) -> Result<bool, GatewayError> {
        let result = sqlx::query("DELETE FROM services WHERE id = ?")
            .bind(id.to_string())
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    // ---------- targets ----------

    pub async fn list_targets(&self) -> Result<Vec<Target>, GatewayError> {
        let rows = sqlx::query(
            "SELECT id, service_id, url, weight, enabled, created_at, updated_at FROM targets",
        )
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(row_to_target).collect()
    }

    pub async fn list_targets_for_service(
        &self,
        service_id: &Uuid,
    ) -> Result<Vec<Target>, GatewayError> {
        let rows = sqlx::query(
            "SELECT id, service_id, url, weight, enabled, created_at, updated_at \
             FROM targets WHERE service_id = ?",
        )
        .bind(service_id.to_string())
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(row_to_target).collect()
    }

    pub async fn get_target(&self, id: &Uuid) -> Result<Option<Target>, GatewayError> {
        let row = sqlx::query(
            "SELECT id, service_id, url, weight, enabled, created_at, updated_at \
             FROM targets WHERE id = ?",
        )
        .bind(id.to_string())
        .fetch_optional(&self.pool)
        .await?;
        row.map(row_to_target).transpose()
    }

    pub async fn create_target(&self, target: &Target) -> Result<(), GatewayError> {
        sqlx::query(
            "INSERT INTO targets (id, service_id, url, weight, enabled, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(target.id.to_string())
        .bind(target.service_id.to_string())
        .bind(&target.url)
        .bind(target.weight as i64)
        .bind(target.enabled as i64)
        .bind(target.created_at.to_rfc3339())
        .bind(target.updated_at.to_rfc3339())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn update_target(&self, id: &Uuid, target: &Target) -> Result<(), GatewayError> {
        sqlx::query(
            "UPDATE targets SET url = ?, weight = ?, enabled = ?, updated_at = ? WHERE id = ?",
        )
        .bind(&target.url)
        .bind(target.weight as i64)
        .bind(target.enabled as i64)
        .bind(target.updated_at.to_rfc3339())
        .bind(id.to_string())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn delete_target(&self, id: &Uuid) -> Result<bool, GatewayError> {
        let result = sqlx::query("DELETE FROM targets WHERE id = ?")
            .bind(id.to_string())
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    // ---------- routes ----------

    pub async fn list_routes(&self) -> Result<Vec<Route>, GatewayError> {
        let rows = sqlx::query(
            "SELECT id, service_id, paths, methods, strip_path, created_at, updated_at \
             FROM routes",
        )
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter().map(row_to_route).collect()
    }

    pub async fn get_route(&self, id: &Uuid) -> Result<Option<Route>, GatewayError> {
        let row = sqlx::query(
            "SELECT id, service_id, paths, methods, strip_path, created_at, updated_at \
             FROM routes WHERE id = ?",
        )
        .bind(id.to_string())
        .fetch_optional(&self.pool)
        .await?;

        row.map(row_to_route).transpose()
    }

    pub async fn create_route(&self, route: &Route) -> Result<(), GatewayError> {
        let paths = serde_json::to_string(&route.paths)
            .map_err(|e| GatewayError::Internal(e.to_string()))?;
        let methods = serde_json::to_string(&route.methods)
            .map_err(|e| GatewayError::Internal(e.to_string()))?;
        let hosts = "[]".to_string();

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

    pub async fn update_route(&self, id: &Uuid, route: &Route) -> Result<(), GatewayError> {
        let paths = serde_json::to_string(&route.paths)
            .map_err(|e| GatewayError::Internal(e.to_string()))?;
        let methods = serde_json::to_string(&route.methods)
            .map_err(|e| GatewayError::Internal(e.to_string()))?;

        sqlx::query(
            "UPDATE routes SET service_id = ?, paths = ?, methods = ?, strip_path = ?, updated_at = ? \
             WHERE id = ?",
        )
        .bind(route.service_id.to_string())
        .bind(paths)
        .bind(methods)
        .bind(route.strip_path as i64)
        .bind(route.updated_at.to_rfc3339())
        .bind(id.to_string())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn delete_route(&self, id: &Uuid) -> Result<bool, GatewayError> {
        let result = sqlx::query("DELETE FROM routes WHERE id = ?")
            .bind(id.to_string())
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    // ---------- plugins ----------

    pub async fn list_plugins(&self) -> Result<Vec<PluginInstance>, GatewayError> {
        let rows = sqlx::query(
            "SELECT id, name, enabled, config, created_at, updated_at FROM plugins",
        )
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter().map(row_to_plugin).collect()
    }

    pub async fn create_plugin(&self, plugin: &PluginInstance) -> Result<(), GatewayError> {
        let config = serde_json::to_string(&plugin.config)
            .map_err(|e| GatewayError::Internal(e.to_string()))?;

        sqlx::query(
            "INSERT INTO plugins (id, name, service_id, route_id, enabled, config, created_at, updated_at) \
             VALUES (?, ?, NULL, NULL, ?, ?, ?, ?)",
        )
        .bind(plugin.id.to_string())
        .bind(&plugin.name)
        .bind(plugin.enabled as i64)
        .bind(config)
        .bind(plugin.created_at.to_rfc3339())
        .bind(plugin.updated_at.to_rfc3339())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn delete_plugin(&self, id: &Uuid) -> Result<bool, GatewayError> {
        let result = sqlx::query("DELETE FROM plugins WHERE id = ?")
            .bind(id.to_string())
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    // ---------- consumers ----------

    pub async fn list_consumers(&self) -> Result<Vec<Consumer>, GatewayError> {
        let rows = sqlx::query(
            "SELECT id, username, custom_id, created_at, updated_at FROM consumers",
        )
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter().map(row_to_consumer).collect()
    }

    pub async fn create_consumer(&self, consumer: &Consumer) -> Result<(), GatewayError> {
        sqlx::query(
            "INSERT INTO consumers (id, username, custom_id, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(consumer.id.to_string())
        .bind(&consumer.username)
        .bind(consumer.custom_id.as_deref())
        .bind(consumer.created_at.to_rfc3339())
        .bind(consumer.updated_at.to_rfc3339())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn delete_consumer(&self, id: &Uuid) -> Result<bool, GatewayError> {
        let result = sqlx::query("DELETE FROM consumers WHERE id = ?")
            .bind(id.to_string())
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    // ---------- bootstrap helpers ----------

    /// Returns true if every persisted table is empty. Used to gate first-boot
    /// seeding from the TOML config.
    pub async fn is_empty(&self) -> Result<bool, GatewayError> {
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

    /// Idempotent seed from the parsed TOML config. Caller should guard with
    /// `is_empty()` so this only runs on a fresh DB.
    pub async fn seed_from_config(&self, config: &GatewayConfig) -> Result<(), GatewayError> {
        // services first so route FKs resolve
        let mut services_by_name = std::collections::HashMap::new();
        for svc_cfg in &config.services {
            let service = Service::new(
                svc_cfg.name.clone(),
                svc_cfg.url.clone(),
                svc_cfg.retries,
                svc_cfg.connect_timeout_ms,
            );
            self.create_service(&service).await?;
            services_by_name.insert(service.name.clone(), service.id);
        }

        for rt_cfg in &config.routes {
            let Some(service_id) = services_by_name.get(&rt_cfg.service).copied() else {
                tracing::warn!(
                    service = %rt_cfg.service,
                    "skipping seed route; referenced service not in config"
                );
                continue;
            };
            let route = Route::new(
                rt_cfg.paths.clone(),
                rt_cfg.methods.clone(),
                service_id,
                rt_cfg.strip_path,
            );
            self.create_route(&route).await?;
        }

        for plugin_cfg in &config.plugins {
            let now = Utc::now();
            // Convert toml::Table to serde_json::Value for storage.
            let config_value =
                serde_json::to_value(&plugin_cfg.config).unwrap_or(serde_json::Value::Null);
            let plugin = PluginInstance {
                id: Uuid::new_v4(),
                name: plugin_cfg.name.clone(),
                enabled: plugin_cfg.enabled,
                config: config_value,
                created_at: now,
                updated_at: now,
            };
            self.create_plugin(&plugin).await?;
        }

        Ok(())
    }
}

// ---------- row mappers ----------

fn parse_uuid(s: &str) -> Result<Uuid, GatewayError> {
    Uuid::from_str(s).map_err(|e| GatewayError::Internal(format!("invalid uuid in db: {e}")))
}

fn parse_dt(s: &str) -> Result<DateTime<Utc>, GatewayError> {
    DateTime::parse_from_rfc3339(s)
        .map(|dt| dt.with_timezone(&Utc))
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

    Ok(Service {
        id: parse_uuid(&id)?,
        name: row.try_get("name")?,
        url: row.try_get("url")?,
        retries: retries as u32,
        connect_timeout_ms: connect_timeout_ms as u64,
        lb_strategy,
        created_at: parse_dt(&created_at)?,
        updated_at: parse_dt(&updated_at)?,
    })
}

fn row_to_target(row: sqlx::sqlite::SqliteRow) -> Result<Target, GatewayError> {
    let id: String = row.try_get("id")?;
    let service_id: String = row.try_get("service_id")?;
    let weight: i64 = row.try_get("weight")?;
    let enabled: i64 = row.try_get("enabled")?;
    let created_at: String = row.try_get("created_at")?;
    let updated_at: String = row.try_get("updated_at")?;

    Ok(Target {
        id: parse_uuid(&id)?,
        service_id: parse_uuid(&service_id)?,
        url: row.try_get("url")?,
        weight: weight as u32,
        enabled: enabled != 0,
        created_at: parse_dt(&created_at)?,
        updated_at: parse_dt(&updated_at)?,
    })
}

fn row_to_route(row: sqlx::sqlite::SqliteRow) -> Result<Route, GatewayError> {
    let id: String = row.try_get("id")?;
    let service_id: String = row.try_get("service_id")?;
    let paths: String = row.try_get("paths")?;
    let methods: String = row.try_get("methods")?;
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
        strip_path: strip_path != 0,
        created_at: parse_dt(&created_at)?,
        updated_at: parse_dt(&updated_at)?,
    })
}

fn row_to_plugin(row: sqlx::sqlite::SqliteRow) -> Result<PluginInstance, GatewayError> {
    let id: String = row.try_get("id")?;
    let enabled: i64 = row.try_get("enabled")?;
    let config: String = row.try_get("config")?;
    let created_at: String = row.try_get("created_at")?;
    let updated_at: String = row.try_get("updated_at")?;

    Ok(PluginInstance {
        id: parse_uuid(&id)?,
        name: row.try_get("name")?,
        enabled: enabled != 0,
        config: serde_json::from_str(&config)
            .map_err(|e| GatewayError::Internal(format!("invalid plugin config json: {e}")))?,
        created_at: parse_dt(&created_at)?,
        updated_at: parse_dt(&updated_at)?,
    })
}

fn row_to_consumer(row: sqlx::sqlite::SqliteRow) -> Result<Consumer, GatewayError> {
    let id: String = row.try_get("id")?;
    let custom_id: Option<String> = row.try_get("custom_id")?;
    let created_at: String = row.try_get("created_at")?;
    let updated_at: String = row.try_get("updated_at")?;

    Ok(Consumer {
        id: parse_uuid(&id)?,
        username: row.try_get("username")?,
        custom_id,
        api_key: None,
        created_at: parse_dt(&created_at)?,
        updated_at: parse_dt(&updated_at)?,
    })
}
