//! Database access for the billing service. Backed by sqlx::AnyPool so
//! the same code runs against sqlite (dev) and postgres (Cloud SQL).
//!
//! Schema convention: every business table uses `id BIGSERIAL`/`INTEGER
//! AUTOINCREMENT` for the internal primary key (never exposed externally)
//! and `urn TEXT` for the external identifier (UUID4 string). All FKs
//! between business tables are BIGINT/INTEGER. INSERTs omit the id and
//! recover it via `RETURNING id` (postgres) / `last_insert_rowid()`
//! (sqlite).

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use chrono::Utc;
use sqlx::Row;
use sqlx::{any::AnyPoolOptions, AnyPool};
use uuid::Uuid;

use super::models::{ApiKeyRecord, Customer, NewApiKey, NewCustomer};

#[async_trait]
pub trait BillingRepo: Send + Sync {
    /// Insert a customer; returns the assigned id.
    async fn create_customer(&self, c: &NewCustomer) -> Result<i64>;
    async fn find_customer_by_email(&self, email: &str) -> Result<Option<Customer>>;
    async fn find_customer(&self, id: i64) -> Result<Option<Customer>>;
    async fn find_customer_by_urn(&self, urn: Uuid) -> Result<Option<Customer>>;
    async fn find_customer_by_stripe(&self, stripe_customer_id: &str) -> Result<Option<Customer>>;
    async fn update_customer_stripe(
        &self,
        id: i64,
        stripe_customer_id: Option<&str>,
        stripe_subscription_id: Option<&str>,
        tier: &str,
        status: &str,
    ) -> Result<()>;

    /// Insert an api key; returns the assigned id.
    async fn insert_api_key(&self, k: &NewApiKey) -> Result<i64>;
    async fn list_api_keys(&self, customer_id: i64) -> Result<Vec<ApiKeyRecord>>;
    async fn find_api_key_by_hash(&self, hash: &str) -> Result<Option<ApiKeyRecord>>;
    async fn revoke_api_key_by_urn(&self, customer_id: i64, urn: Uuid) -> Result<bool>;
    async fn revoke_all_for_customer(&self, customer_id: i64) -> Result<()>;
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

// sqlite stores timestamps as ISO-8601 TEXT; postgres uses TIMESTAMPTZ.
fn parse_dt(s: &str) -> Result<chrono::DateTime<chrono::Utc>> {
    Ok(chrono::DateTime::parse_from_rfc3339(s)
        .map_err(|e| anyhow!("dt parse: {e}"))?
        .with_timezone(&chrono::Utc))
}

// sqlx::Any does NOT rewrite `?` to `$N` for postgres
// (https://github.com/launchbadge/sqlx/issues/2773). Two markers:
//   `?`   ordinary placeholder — `$N` for postgres, `?` for sqlite
//   `?@`  timestamp placeholder — `$N::timestamptz` for postgres, `?` for sqlite
// We bind RFC-3339 strings for timestamps; postgres needs the cast since
// the column is TIMESTAMPTZ and sqlx::Any doesn't impl Type<Any> for
// chrono types.
fn ph(query: &str, is_postgres: bool) -> String {
    let mut out = String::with_capacity(query.len() + 16);
    let mut idx = 0;
    let mut chars = query.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '?' {
            idx += 1;
            let is_ts = chars.peek() == Some(&'@');
            if is_ts {
                chars.next();
            }
            if is_postgres {
                out.push('$');
                out.push_str(&idx.to_string());
                if is_ts {
                    out.push_str("::timestamptz");
                }
            } else {
                out.push('?');
            }
        } else {
            out.push(ch);
        }
    }
    out
}

// is_deleted is BOOLEAN on postgres and INTEGER 0/1 on sqlite.
// Always bind bool — sqlx::Any maps it correctly per backend.
fn read_id(row: &sqlx::any::AnyRow) -> Result<i64> {
    row.try_get::<i64, _>("id")
        .map_err(|e| anyhow!("read id: {e}"))
}

// SELECT column lists — postgres TIMESTAMPTZ columns are formatted to
// RFC-3339 TEXT via to_char so chrono::DateTime::parse_from_rfc3339
// accepts them. (`::text` would produce '2026-05-06 22:34:32+00' which
// is NOT RFC-3339 and parse_from_rfc3339 rejects it.) Sqlite columns
// are stored as TEXT and read directly.
const PG_DT_FMT: &str = "YYYY-MM-DD\"T\"HH24:MI:SS.US\"+00:00\"";

fn select_customer_cols(is_pg: bool) -> String {
    if is_pg {
        format!(
            "id, urn, email, password_hash, name, company, tier, \
             stripe_customer_id, stripe_subscription_id, subscription_status, \
             is_deleted, \
             to_char(created_at AT TIME ZONE 'UTC', '{f}') AS created_at, \
             to_char(updated_at AT TIME ZONE 'UTC', '{f}') AS updated_at",
            f = PG_DT_FMT,
        )
    } else {
        "id, urn, email, password_hash, name, company, tier, \
         stripe_customer_id, stripe_subscription_id, subscription_status, \
         is_deleted, created_at, updated_at"
            .to_string()
    }
}

fn select_api_key_cols(is_pg: bool) -> String {
    if is_pg {
        format!(
            "id, urn, customer_id, name, key_hash, key_prefix, status, \
             to_char(expires_at AT TIME ZONE 'UTC', '{f}') AS expires_at, \
             is_deleted, \
             to_char(created_at AT TIME ZONE 'UTC', '{f}') AS created_at, \
             to_char(updated_at AT TIME ZONE 'UTC', '{f}') AS updated_at, \
             to_char(last_used_at AT TIME ZONE 'UTC', '{f}') AS last_used_at",
            f = PG_DT_FMT,
        )
    } else {
        "id, urn, customer_id, name, key_hash, key_prefix, status, \
         expires_at, is_deleted, created_at, updated_at, last_used_at"
            .to_string()
    }
}

#[async_trait]
impl BillingRepo for SqlBillingRepo {
    async fn create_customer(&self, c: &NewCustomer) -> Result<i64> {
        // INSERT and capture the assigned id. Postgres uses RETURNING;
        // sqlite uses last_insert_rowid via fetch_one+RETURNING (sqlite
        // 3.35+ supports RETURNING — Cargo's bundled sqlx-sqlite uses it).
        let row = sqlx::query(&ph(
            "INSERT INTO customers \
             (urn, email, password_hash, name, company, tier, \
              stripe_customer_id, stripe_subscription_id, subscription_status, \
              is_deleted, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?@, ?@) \
             RETURNING id",
            self.is_postgres,
        ))
        .bind(c.urn.to_string())
        .bind(&c.email)
        .bind(&c.password_hash)
        .bind(c.name.as_deref())
        .bind(c.company.as_deref())
        .bind(&c.tier)
        .bind(c.stripe_customer_id.as_deref())
        .bind(c.stripe_subscription_id.as_deref())
        .bind(&c.subscription_status)
        .bind(false)
        .bind(Utc::now().to_rfc3339())
        .bind(Utc::now().to_rfc3339())
        .fetch_one(&self.pool)
        .await?;
        read_id(&row)
    }

    async fn find_customer_by_email(&self, email: &str) -> Result<Option<Customer>> {
        let row = sqlx::query(&ph(
            &format!(
                "SELECT {} FROM customers WHERE email = ? AND is_deleted = ?",
                select_customer_cols(self.is_postgres)
            ),
            self.is_postgres,
        ))
        .bind(email)
        .bind(false)
        .fetch_optional(&self.pool)
        .await?;
        row.map(row_to_customer).transpose()
    }

    async fn find_customer(&self, id: i64) -> Result<Option<Customer>> {
        let row = sqlx::query(&ph(
            &format!(
                "SELECT {} FROM customers WHERE id = ? AND is_deleted = ?",
                select_customer_cols(self.is_postgres)
            ),
            self.is_postgres,
        ))
        .bind(id)
        .bind(false)
        .fetch_optional(&self.pool)
        .await?;
        row.map(row_to_customer).transpose()
    }

    async fn find_customer_by_urn(&self, urn: Uuid) -> Result<Option<Customer>> {
        let row = sqlx::query(&ph(
            &format!(
                "SELECT {} FROM customers WHERE urn = ? AND is_deleted = ?",
                select_customer_cols(self.is_postgres)
            ),
            self.is_postgres,
        ))
        .bind(urn.to_string())
        .bind(false)
        .fetch_optional(&self.pool)
        .await?;
        row.map(row_to_customer).transpose()
    }

    async fn find_customer_by_stripe(&self, stripe_customer_id: &str) -> Result<Option<Customer>> {
        let row = sqlx::query(&ph(
            &format!(
                "SELECT {} FROM customers WHERE stripe_customer_id = ? AND is_deleted = ?",
                select_customer_cols(self.is_postgres)
            ),
            self.is_postgres,
        ))
        .bind(stripe_customer_id)
        .bind(false)
        .fetch_optional(&self.pool)
        .await?;
        row.map(row_to_customer).transpose()
    }

    async fn update_customer_stripe(
        &self,
        id: i64,
        stripe_customer_id: Option<&str>,
        stripe_subscription_id: Option<&str>,
        tier: &str,
        status: &str,
    ) -> Result<()> {
        sqlx::query(&ph(
            "UPDATE customers SET stripe_customer_id = COALESCE(?, stripe_customer_id), \
             stripe_subscription_id = ?, tier = ?, subscription_status = ?, updated_at = ?@ \
             WHERE id = ?",
            self.is_postgres,
        ))
        .bind(stripe_customer_id)
        .bind(stripe_subscription_id)
        .bind(tier)
        .bind(status)
        .bind(Utc::now().to_rfc3339())
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn insert_api_key(&self, k: &NewApiKey) -> Result<i64> {
        let row = sqlx::query(&ph(
            "INSERT INTO api_keys \
             (urn, customer_id, name, key_hash, key_prefix, status, \
              expires_at, is_deleted, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?@, ?, ?@, ?@) \
             RETURNING id",
            self.is_postgres,
        ))
        .bind(k.urn.to_string())
        .bind(k.customer_id)
        .bind(&k.name)
        .bind(&k.key_hash)
        .bind(&k.key_prefix)
        .bind(&k.status)
        .bind(k.expires_at.map(|t| t.to_rfc3339()))
        .bind(false)
        .bind(Utc::now().to_rfc3339())
        .bind(Utc::now().to_rfc3339())
        .fetch_one(&self.pool)
        .await?;
        read_id(&row)
    }

    async fn list_api_keys(&self, customer_id: i64) -> Result<Vec<ApiKeyRecord>> {
        let rows = sqlx::query(&ph(
            &format!("SELECT {} FROM api_keys WHERE customer_id = ? AND is_deleted = ? ORDER BY created_at DESC", select_api_key_cols(self.is_postgres)),
            self.is_postgres,
        ))
        .bind(customer_id)
        .bind(false)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(row_to_api_key).collect()
    }

    async fn find_api_key_by_hash(&self, hash: &str) -> Result<Option<ApiKeyRecord>> {
        let row = sqlx::query(&ph(
            &format!(
                "SELECT {} FROM api_keys WHERE key_hash = ? AND is_deleted = ?",
                select_api_key_cols(self.is_postgres)
            ),
            self.is_postgres,
        ))
        .bind(hash)
        .bind(false)
        .fetch_optional(&self.pool)
        .await?;
        row.map(row_to_api_key).transpose()
    }

    async fn revoke_api_key_by_urn(&self, customer_id: i64, urn: Uuid) -> Result<bool> {
        // Soft-delete: flip is_deleted AND status='revoked'.
        let res = sqlx::query(&ph(
            "UPDATE api_keys SET status = 'revoked', is_deleted = ?, updated_at = ?@ \
             WHERE urn = ? AND customer_id = ?",
            self.is_postgres,
        ))
        .bind(true)
        .bind(Utc::now().to_rfc3339())
        .bind(urn.to_string())
        .bind(customer_id)
        .execute(&self.pool)
        .await?;
        Ok(res.rows_affected() > 0)
    }

    async fn revoke_all_for_customer(&self, customer_id: i64) -> Result<()> {
        sqlx::query(&ph(
            "UPDATE api_keys SET status = 'revoked', is_deleted = ?, updated_at = ?@ \
             WHERE customer_id = ?",
            self.is_postgres,
        ))
        .bind(true)
        .bind(Utc::now().to_rfc3339())
        .bind(customer_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}

fn parse_uuid(s: &str) -> Result<Uuid> {
    Uuid::parse_str(s).map_err(|e| anyhow!("uuid parse: {e}"))
}

// sqlx::Any doesn't impl Decode<Any> for chrono types, so timestamps are
// read as String. SELECTs cast TIMESTAMPTZ → TEXT in postgres queries
// (see ph_select). For sqlite the column is already TEXT.
fn read_dt(row: &sqlx::any::AnyRow, col: &str) -> Result<chrono::DateTime<chrono::Utc>> {
    let s: String = row.try_get(col).map_err(|e| anyhow!("read {col}: {e}"))?;
    parse_dt(&s)
}

fn read_dt_opt(
    row: &sqlx::any::AnyRow,
    col: &str,
) -> Result<Option<chrono::DateTime<chrono::Utc>>> {
    let s: Option<String> = row.try_get(col).ok().flatten();
    s.as_deref().map(parse_dt).transpose()
}

fn row_to_customer(row: sqlx::any::AnyRow) -> Result<Customer> {
    let urn: String = row.try_get("urn")?;
    Ok(Customer {
        id: row.try_get::<i64, _>("id")?,
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
        is_deleted: row
            .try_get::<bool, _>("is_deleted")
            .or_else(|_| row.try_get::<i32, _>("is_deleted").map(|v| v != 0))
            .unwrap_or(false),
        created_at: read_dt(&row, "created_at")?,
        updated_at: read_dt(&row, "updated_at")?,
    })
}

fn row_to_api_key(row: sqlx::any::AnyRow) -> Result<ApiKeyRecord> {
    let urn: String = row.try_get("urn")?;
    Ok(ApiKeyRecord {
        id: row.try_get::<i64, _>("id")?,
        urn: parse_uuid(&urn)?,
        customer_id: row.try_get::<i64, _>("customer_id")?,
        name: row.try_get("name")?,
        key_hash: row.try_get("key_hash")?,
        key_prefix: row.try_get("key_prefix")?,
        status: row.try_get("status")?,
        expires_at: read_dt_opt(&row, "expires_at")?,
        is_deleted: row
            .try_get::<bool, _>("is_deleted")
            .or_else(|_| row.try_get::<i32, _>("is_deleted").map(|v| v != 0))
            .unwrap_or(false),
        created_at: read_dt(&row, "created_at")?,
        updated_at: read_dt(&row, "updated_at")?,
        last_used_at: read_dt_opt(&row, "last_used_at")?,
    })
}
