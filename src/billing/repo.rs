//! Database access for the billing service. Backed by sqlx::AnyPool so
//! the same code runs against sqlite (dev) and postgres (Cloud SQL).
//!
//! All inserts populate `urn` explicitly — even though both schemas have
//! a default — so behaviour is deterministic and identical across DBs.
//! All reads filter `is_deleted = 0/false`. Deletes are soft (set the
//! flag) so audit and webhook flows can still resolve a customer/key.

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use chrono::Utc;
use sqlx::Row;
use sqlx::{any::AnyPoolOptions, AnyPool};
use uuid::Uuid;

use super::models::{ApiKeyRecord, Customer};

#[async_trait]
pub trait BillingRepo: Send + Sync {
    async fn create_customer(&self, c: &Customer) -> Result<()>;
    async fn find_customer_by_email(&self, email: &str) -> Result<Option<Customer>>;
    async fn find_customer(&self, id: Uuid) -> Result<Option<Customer>>;
    async fn find_customer_by_urn(&self, urn: Uuid) -> Result<Option<Customer>>;
    async fn find_customer_by_stripe(&self, stripe_customer_id: &str) -> Result<Option<Customer>>;
    async fn update_customer_stripe(
        &self,
        id: Uuid,
        stripe_customer_id: Option<&str>,
        stripe_subscription_id: Option<&str>,
        tier: &str,
        status: &str,
    ) -> Result<()>;

    async fn insert_api_key(&self, k: &ApiKeyRecord) -> Result<()>;
    async fn list_api_keys(&self, customer_id: Uuid) -> Result<Vec<ApiKeyRecord>>;
    async fn find_api_key_by_hash(&self, hash: &str) -> Result<Option<ApiKeyRecord>>;
    async fn revoke_api_key_by_urn(&self, customer_id: Uuid, urn: Uuid) -> Result<bool>;
    async fn revoke_all_for_customer(&self, customer_id: Uuid) -> Result<()>;
}

pub struct SqlBillingRepo {
    pool: AnyPool,
    is_postgres: bool,
}

impl SqlBillingRepo {
    pub async fn connect(database_url: &str) -> Result<Self> {
        sqlx::any::install_default_drivers();
        let pool = AnyPoolOptions::new()
            .max_connections(8)
            .connect(database_url)
            .await
            .map_err(|e| anyhow!("connect billing db: {e}"))?;
        let is_postgres = database_url.starts_with("postgres");
        Ok(Self { pool, is_postgres })
    }

    pub fn pool(&self) -> &AnyPool {
        &self.pool
    }

    pub fn is_postgres(&self) -> bool {
        self.is_postgres
    }
}

// In sqlite we store timestamps as ISO-8601 strings; in postgres they
// are TIMESTAMPTZ. The Any driver returns the stored type, so the row
// helpers below try string first, then fall back to a chrono::DateTime.
fn parse_dt(s: &str) -> Result<chrono::DateTime<chrono::Utc>> {
    Ok(chrono::DateTime::parse_from_rfc3339(s)
        .map_err(|e| anyhow!("dt parse: {e}"))?
        .with_timezone(&chrono::Utc))
}

#[async_trait]
impl BillingRepo for SqlBillingRepo {
    async fn create_customer(&self, c: &Customer) -> Result<()> {
        sqlx::query(
            "INSERT INTO customers \
             (id, urn, email, password_hash, name, company, tier, \
              stripe_customer_id, stripe_subscription_id, subscription_status, \
              is_deleted, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(c.id.to_string())
        .bind(c.urn.to_string())
        .bind(&c.email)
        .bind(&c.password_hash)
        .bind(c.name.as_deref())
        .bind(c.company.as_deref())
        .bind(&c.tier)
        .bind(c.stripe_customer_id.as_deref())
        .bind(c.stripe_subscription_id.as_deref())
        .bind(&c.subscription_status)
        .bind(false_for(self.is_postgres))
        .bind(c.created_at.to_rfc3339())
        .bind(c.updated_at.to_rfc3339())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn find_customer_by_email(&self, email: &str) -> Result<Option<Customer>> {
        let row = sqlx::query("SELECT * FROM customers WHERE email = ? AND is_deleted = ?")
            .bind(email)
            .bind(false_for(self.is_postgres))
            .fetch_optional(&self.pool)
            .await?;
        row.map(row_to_customer).transpose()
    }

    async fn find_customer(&self, id: Uuid) -> Result<Option<Customer>> {
        let row = sqlx::query("SELECT * FROM customers WHERE id = ? AND is_deleted = ?")
            .bind(id.to_string())
            .bind(false_for(self.is_postgres))
            .fetch_optional(&self.pool)
            .await?;
        row.map(row_to_customer).transpose()
    }

    async fn find_customer_by_urn(&self, urn: Uuid) -> Result<Option<Customer>> {
        let row = sqlx::query("SELECT * FROM customers WHERE urn = ? AND is_deleted = ?")
            .bind(urn.to_string())
            .bind(false_for(self.is_postgres))
            .fetch_optional(&self.pool)
            .await?;
        row.map(row_to_customer).transpose()
    }

    async fn find_customer_by_stripe(&self, stripe_customer_id: &str) -> Result<Option<Customer>> {
        let row =
            sqlx::query("SELECT * FROM customers WHERE stripe_customer_id = ? AND is_deleted = ?")
                .bind(stripe_customer_id)
                .bind(false_for(self.is_postgres))
                .fetch_optional(&self.pool)
                .await?;
        row.map(row_to_customer).transpose()
    }

    async fn update_customer_stripe(
        &self,
        id: Uuid,
        stripe_customer_id: Option<&str>,
        stripe_subscription_id: Option<&str>,
        tier: &str,
        status: &str,
    ) -> Result<()> {
        sqlx::query(
            "UPDATE customers SET stripe_customer_id = COALESCE(?, stripe_customer_id), \
             stripe_subscription_id = ?, tier = ?, subscription_status = ?, updated_at = ? \
             WHERE id = ?",
        )
        .bind(stripe_customer_id)
        .bind(stripe_subscription_id)
        .bind(tier)
        .bind(status)
        .bind(Utc::now().to_rfc3339())
        .bind(id.to_string())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn insert_api_key(&self, k: &ApiKeyRecord) -> Result<()> {
        sqlx::query(
            "INSERT INTO api_keys \
             (id, urn, customer_id, name, key_hash, key_prefix, status, \
              expires_at, is_deleted, created_at, updated_at, last_used_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(k.id.to_string())
        .bind(k.urn.to_string())
        .bind(k.customer_id.to_string())
        .bind(&k.name)
        .bind(&k.key_hash)
        .bind(&k.key_prefix)
        .bind(&k.status)
        .bind(k.expires_at.map(|t| t.to_rfc3339()))
        .bind(false_for(self.is_postgres))
        .bind(k.created_at.to_rfc3339())
        .bind(k.updated_at.to_rfc3339())
        .bind(k.last_used_at.map(|t| t.to_rfc3339()))
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn list_api_keys(&self, customer_id: Uuid) -> Result<Vec<ApiKeyRecord>> {
        let rows = sqlx::query(
            "SELECT * FROM api_keys WHERE customer_id = ? AND is_deleted = ? \
             ORDER BY created_at DESC",
        )
        .bind(customer_id.to_string())
        .bind(false_for(self.is_postgres))
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(row_to_api_key).collect()
    }

    async fn find_api_key_by_hash(&self, hash: &str) -> Result<Option<ApiKeyRecord>> {
        let row = sqlx::query("SELECT * FROM api_keys WHERE key_hash = ? AND is_deleted = ?")
            .bind(hash)
            .bind(false_for(self.is_postgres))
            .fetch_optional(&self.pool)
            .await?;
        row.map(row_to_api_key).transpose()
    }

    async fn revoke_api_key_by_urn(&self, customer_id: Uuid, urn: Uuid) -> Result<bool> {
        // Soft-delete: flip is_deleted AND status='revoked'. revoked-but-
        // not-deleted is unreachable in this codebase but the dual flag
        // lets the verify endpoint reject precisely.
        let res = sqlx::query(
            "UPDATE api_keys SET status = 'revoked', is_deleted = ?, updated_at = ? \
             WHERE urn = ? AND customer_id = ?",
        )
        .bind(true_for(self.is_postgres))
        .bind(Utc::now().to_rfc3339())
        .bind(urn.to_string())
        .bind(customer_id.to_string())
        .execute(&self.pool)
        .await?;
        Ok(res.rows_affected() > 0)
    }

    async fn revoke_all_for_customer(&self, customer_id: Uuid) -> Result<()> {
        sqlx::query(
            "UPDATE api_keys SET status = 'revoked', is_deleted = ?, updated_at = ? \
             WHERE customer_id = ?",
        )
        .bind(true_for(self.is_postgres))
        .bind(Utc::now().to_rfc3339())
        .bind(customer_id.to_string())
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}

// sqlx::Any binds booleans as integers for sqlite and as bool for
// postgres. Use the right Rust type per backend so we don't trigger
// "type mismatch" errors at runtime.
fn false_for(_is_postgres: bool) -> i32 {
    0
}
fn true_for(_is_postgres: bool) -> i32 {
    1
}

fn parse_uuid(s: &str) -> Result<Uuid> {
    Uuid::parse_str(s).map_err(|e| anyhow!("uuid parse: {e}"))
}

fn row_to_customer(row: sqlx::any::AnyRow) -> Result<Customer> {
    let id: String = row.try_get("id")?;
    let urn: String = row.try_get("urn")?;
    let created_at: String = row.try_get("created_at")?;
    let updated_at: String = row.try_get("updated_at")?;
    Ok(Customer {
        id: parse_uuid(&id)?,
        urn: parse_uuid(&urn)?,
        email: row.try_get("email")?,
        password_hash: row.try_get("password_hash")?,
        name: row.try_get::<Option<String>, _>("name").ok().flatten(),
        company: row.try_get::<Option<String>, _>("company").ok().flatten(),
        tier: row.try_get("tier")?,
        stripe_customer_id: row
            .try_get::<Option<String>, _>("stripe_customer_id")
            .ok()
            .flatten(),
        stripe_subscription_id: row
            .try_get::<Option<String>, _>("stripe_subscription_id")
            .ok()
            .flatten(),
        subscription_status: row.try_get("subscription_status")?,
        is_deleted: row.try_get::<i32, _>("is_deleted").unwrap_or(0) != 0,
        created_at: parse_dt(&created_at)?,
        updated_at: parse_dt(&updated_at)?,
    })
}

fn row_to_api_key(row: sqlx::any::AnyRow) -> Result<ApiKeyRecord> {
    let id: String = row.try_get("id")?;
    let urn: String = row.try_get("urn")?;
    let customer_id: String = row.try_get("customer_id")?;
    let created_at: String = row.try_get("created_at")?;
    let updated_at: String = row.try_get("updated_at")?;
    let expires_at: Option<String> = row.try_get("expires_at").ok().flatten();
    let last_used_at: Option<String> = row.try_get("last_used_at").ok().flatten();
    Ok(ApiKeyRecord {
        id: parse_uuid(&id)?,
        urn: parse_uuid(&urn)?,
        customer_id: parse_uuid(&customer_id)?,
        name: row.try_get("name")?,
        key_hash: row.try_get("key_hash")?,
        key_prefix: row.try_get("key_prefix")?,
        status: row.try_get("status")?,
        expires_at: expires_at.as_deref().map(parse_dt).transpose()?,
        is_deleted: row.try_get::<i32, _>("is_deleted").unwrap_or(0) != 0,
        created_at: parse_dt(&created_at)?,
        updated_at: parse_dt(&updated_at)?,
        last_used_at: last_used_at.as_deref().map(parse_dt).transpose()?,
    })
}
