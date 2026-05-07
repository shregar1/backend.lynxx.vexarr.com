//! Database access for the billing service. Backed by sqlx::AnyPool so
//! the same code runs against sqlite (dev) and postgres (Cloud SQL).
//!
//! Schema convention: every business table uses `id BIGSERIAL`/`INTEGER
//! AUTOINCREMENT` for the internal primary key (never exposed externally)
//! and `urn TEXT` for the external identifier (UUID4 string). All FKs
//! between business tables are BIGINT/INTEGER. INSERTs omit the id and
//! recover it via `RETURNING id` (postgres) / `last_insert_rowid()`
//! (sqlite).
//!
//! Enum values are stored exclusively as FK integers referencing lookup
//! tables (`billing_tier_lk`, `subscription_status_lk`,
//! `subscription_type_lk`, `api_key_status_lk`). SELECTs JOIN those
//! tables to resolve the human-readable `code` column for use in
//! business logic and API responses.

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use chrono::Utc;
use serde_json;
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
    /// Update Stripe-related fields and the tier/status FKs.
    /// `tier_code` and `status_code` are lookup-table codes resolved to
    /// FK IDs via subquery.
    async fn update_customer_stripe(
        &self,
        id: i64,
        stripe_customer_id: Option<&str>,
        stripe_subscription_id: Option<&str>,
        tier_code: &str,
        status_code: &str,
    ) -> Result<()>;
    /// Update subscription type FK (e.g. free → paid, free → internal).
    async fn update_customer_subscription_type(
        &self,
        id: i64,
        subscription_type_code: &str,
    ) -> Result<()>;

    /// Insert an api key; returns the assigned id.
    async fn insert_api_key(&self, k: &NewApiKey) -> Result<i64>;
    async fn list_api_keys(&self, customer_id: i64) -> Result<Vec<ApiKeyRecord>>;
    /// Count active (non-deleted, status=active) keys for a customer.
    async fn count_active_keys(&self, customer_id: i64) -> Result<i64>;
    async fn find_api_key_by_hash(&self, hash: &str) -> Result<Option<ApiKeyRecord>>;
    /// Revoke a key by URN. Returns Ok(true) if newly revoked, Ok(false)
    /// if not found. Returns Err if already revoked (caller maps to 409).
    async fn revoke_api_key_by_urn(&self, customer_id: i64, urn: Uuid) -> Result<bool>;
    async fn revoke_all_for_customer(&self, customer_id: i64) -> Result<()>;
    /// Touch last_used_at for a key (called from /v1/license/verify).
    async fn touch_last_used(&self, key_id: i64) -> Result<()>;
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
// (https://github.com/launchbadge/sqlx/issues/2773). Three markers:
//   `?`   ordinary placeholder — `$N` for postgres, `?` for sqlite
//   `?@`  timestamp placeholder — `$N::timestamptz` for postgres, `?` for sqlite
//   `?#`  text-array placeholder — `$N::text[]` for postgres, `?` for sqlite
// We bind RFC-3339 strings for timestamps and `{a,b,c}` text-array
// literals for arrays; postgres needs the cast since sqlx::Any doesn't
// impl Type<Any> for chrono types or Vec<String>.
fn ph(query: &str, is_postgres: bool) -> String {
    let mut out = String::with_capacity(query.len() + 16);
    let mut idx = 0;
    let mut chars = query.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '?' {
            idx += 1;
            let next = chars.peek().copied();
            let cast = match next {
                Some('@') => {
                    chars.next();
                    Some("::timestamptz")
                }
                Some('#') => {
                    chars.next();
                    Some("::text[]")
                }
                _ => None,
            };
            if is_postgres {
                out.push('$');
                out.push_str(&idx.to_string());
                if let Some(c) = cast {
                    out.push_str(c);
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
// accepts them. Sqlite columns are stored as TEXT and read directly.
//
// JOINs resolve the FK columns to their lookup-table `code` values.
const PG_DT_FMT: &str = "YYYY-MM-DD\"T\"HH24:MI:SS.US\"+00:00\"";

fn select_customer_sql(is_pg: bool) -> String {
    if is_pg {
        format!(
            "SELECT c.id, c.urn, c.email, c.password_hash, c.name, c.company, \
             c.billing_tier_id, bt.code AS tier_code, \
             c.stripe_customer_id, c.stripe_subscription_id, \
             c.subscription_status_id, ss.code AS subscription_status_code, \
             c.subscription_type_id, st.code AS subscription_type_code, \
             c.is_deleted, \
             to_char(c.created_at AT TIME ZONE 'UTC', '{f}') AS created_at, \
             to_char(c.updated_at AT TIME ZONE 'UTC', '{f}') AS updated_at \
             FROM customers c \
             JOIN billing_tier_lk bt ON bt.id = c.billing_tier_id \
             JOIN subscription_status_lk ss ON ss.id = c.subscription_status_id \
             JOIN subscription_type_lk st ON st.id = c.subscription_type_id",
            f = PG_DT_FMT,
        )
    } else {
        "SELECT c.id, c.urn, c.email, c.password_hash, c.name, c.company, \
         c.billing_tier_id, bt.code AS tier_code, \
         c.stripe_customer_id, c.stripe_subscription_id, \
         c.subscription_status_id, ss.code AS subscription_status_code, \
         c.subscription_type_id, st.code AS subscription_type_code, \
         c.is_deleted, c.created_at, c.updated_at \
         FROM customers c \
         JOIN billing_tier_lk bt ON bt.id = c.billing_tier_id \
         JOIN subscription_status_lk ss ON ss.id = c.subscription_status_id \
         JOIN subscription_type_lk st ON st.id = c.subscription_type_id"
            .to_string()
    }
}

fn select_api_key_sql(is_pg: bool) -> String {
    if is_pg {
        // Cast scopes to text so sqlx::Any can read it as a String —
        // its `Any` row decoder doesn't handle postgres TEXT[] natively.
        // The text form is `{read,write}` which parse_scopes() splits.
        format!(
            "SELECT k.id, k.urn, k.customer_id, k.name, k.key_hash, k.key_prefix, \
             k.api_key_status_id, aks.code AS status_code, \
             k.scopes::text AS scopes, \
             to_char(k.expires_at AT TIME ZONE 'UTC', '{f}') AS expires_at, \
             to_char(k.revoked_at AT TIME ZONE 'UTC', '{f}') AS revoked_at, \
             k.is_deleted, \
             to_char(k.created_at AT TIME ZONE 'UTC', '{f}') AS created_at, \
             to_char(k.updated_at AT TIME ZONE 'UTC', '{f}') AS updated_at, \
             to_char(k.last_used_at AT TIME ZONE 'UTC', '{f}') AS last_used_at \
             FROM api_keys k \
             JOIN api_key_status_lk aks ON aks.id = k.api_key_status_id",
            f = PG_DT_FMT,
        )
    } else {
        "SELECT k.id, k.urn, k.customer_id, k.name, k.key_hash, k.key_prefix, \
         k.api_key_status_id, aks.code AS status_code, \
         k.scopes, \
         k.expires_at, k.revoked_at, k.is_deleted, k.created_at, k.updated_at, k.last_used_at \
         FROM api_keys k \
         JOIN api_key_status_lk aks ON aks.id = k.api_key_status_id"
            .to_string()
    }
}

#[async_trait]
impl BillingRepo for SqlBillingRepo {
    async fn create_customer(&self, c: &NewCustomer) -> Result<i64> {
        // INSERT resolves codes → FK IDs via subquery so the repo never
        // needs to hard-code lookup-table row IDs.
        let row = sqlx::query(&ph(
            "INSERT INTO customers \
             (urn, email, password_hash, name, company, \
              billing_tier_id, \
              stripe_customer_id, stripe_subscription_id, \
              subscription_status_id, subscription_type_id, \
              is_deleted, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, \
              (SELECT id FROM billing_tier_lk WHERE code = ?), \
              ?, ?, \
              (SELECT id FROM subscription_status_lk WHERE code = ?), \
              (SELECT id FROM subscription_type_lk WHERE code = ?), \
              ?, ?@, ?@) \
             RETURNING id",
            self.is_postgres,
        ))
        .bind(c.urn.to_string())
        .bind(&c.email)
        .bind(&c.password_hash)
        .bind(c.name.as_deref())
        .bind(c.company.as_deref())
        .bind(&c.tier_code)
        .bind(c.stripe_customer_id.as_deref())
        .bind(c.stripe_subscription_id.as_deref())
        .bind(&c.subscription_status_code)
        .bind(&c.subscription_type_code)
        .bind(false)
        .bind(Utc::now().to_rfc3339())
        .bind(Utc::now().to_rfc3339())
        .fetch_one(&self.pool)
        .await?;
        read_id(&row)
    }

    async fn find_customer_by_email(&self, email: &str) -> Result<Option<Customer>> {
        let sql = format!(
            "{} WHERE c.email = ? AND c.is_deleted = ?",
            select_customer_sql(self.is_postgres)
        );
        let row = sqlx::query(&ph(&sql, self.is_postgres))
            .bind(email)
            .bind(false)
            .fetch_optional(&self.pool)
            .await?;
        row.map(row_to_customer).transpose()
    }

    async fn find_customer(&self, id: i64) -> Result<Option<Customer>> {
        let sql = format!(
            "{} WHERE c.id = ? AND c.is_deleted = ?",
            select_customer_sql(self.is_postgres)
        );
        let row = sqlx::query(&ph(&sql, self.is_postgres))
            .bind(id)
            .bind(false)
            .fetch_optional(&self.pool)
            .await?;
        row.map(row_to_customer).transpose()
    }

    async fn find_customer_by_urn(&self, urn: Uuid) -> Result<Option<Customer>> {
        let sql = format!(
            "{} WHERE c.urn = ? AND c.is_deleted = ?",
            select_customer_sql(self.is_postgres)
        );
        let row = sqlx::query(&ph(&sql, self.is_postgres))
            .bind(urn.to_string())
            .bind(false)
            .fetch_optional(&self.pool)
            .await?;
        row.map(row_to_customer).transpose()
    }

    async fn find_customer_by_stripe(&self, stripe_customer_id: &str) -> Result<Option<Customer>> {
        let sql = format!(
            "{} WHERE c.stripe_customer_id = ? AND c.is_deleted = ?",
            select_customer_sql(self.is_postgres)
        );
        let row = sqlx::query(&ph(&sql, self.is_postgres))
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
        tier_code: &str,
        status_code: &str,
    ) -> Result<()> {
        sqlx::query(&ph(
            "UPDATE customers SET \
             stripe_customer_id = COALESCE(?, stripe_customer_id), \
             stripe_subscription_id = ?, \
             billing_tier_id = (SELECT id FROM billing_tier_lk WHERE code = ?), \
             subscription_status_id = (SELECT id FROM subscription_status_lk WHERE code = ?), \
             updated_at = ?@ \
             WHERE id = ?",
            self.is_postgres,
        ))
        .bind(stripe_customer_id)
        .bind(stripe_subscription_id)
        .bind(tier_code)
        .bind(status_code)
        .bind(Utc::now().to_rfc3339())
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn update_customer_subscription_type(
        &self,
        id: i64,
        subscription_type_code: &str,
    ) -> Result<()> {
        sqlx::query(&ph(
            "UPDATE customers SET \
             subscription_type_id = (SELECT id FROM subscription_type_lk WHERE code = ?), \
             updated_at = ?@ \
             WHERE id = ?",
            self.is_postgres,
        ))
        .bind(subscription_type_code)
        .bind(Utc::now().to_rfc3339())
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn insert_api_key(&self, k: &NewApiKey) -> Result<i64> {
        // Postgres uses TEXT[] for scopes; sqlite uses JSON text.
        let scopes_value = if self.is_postgres {
            // Postgres TEXT[] literal: {read,write}
            format!(
                "{{{}}}",
                k.scopes
                    .iter()
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join(",")
            )
        } else {
            serde_json::to_string(&k.scopes).unwrap_or_else(|_| "[]".to_string())
        };
        let row = sqlx::query(&ph(
            "INSERT INTO api_keys \
             (urn, customer_id, name, key_hash, key_prefix, \
              api_key_status_id, scopes, \
              expires_at, is_deleted, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, \
              (SELECT id FROM api_key_status_lk WHERE code = ?), ?#, \
              ?@, ?, ?@, ?@) \
             RETURNING id",
            self.is_postgres,
        ))
        .bind(k.urn.to_string())
        .bind(k.customer_id)
        .bind(&k.name)
        .bind(&k.key_hash)
        .bind(&k.key_prefix)
        .bind(&k.status_code)
        .bind(&scopes_value)
        .bind(k.expires_at.map(|t| t.to_rfc3339()))
        .bind(false)
        .bind(Utc::now().to_rfc3339())
        .bind(Utc::now().to_rfc3339())
        .fetch_one(&self.pool)
        .await?;
        read_id(&row)
    }

    async fn list_api_keys(&self, customer_id: i64) -> Result<Vec<ApiKeyRecord>> {
        let sql = format!(
            "{} WHERE k.customer_id = ? AND k.is_deleted = ? ORDER BY k.created_at DESC",
            select_api_key_sql(self.is_postgres)
        );
        let rows = sqlx::query(&ph(&sql, self.is_postgres))
            .bind(customer_id)
            .bind(false)
            .fetch_all(&self.pool)
            .await?;
        rows.into_iter()
            .map(|r| row_to_api_key(r, self.is_postgres))
            .collect()
    }

    async fn count_active_keys(&self, customer_id: i64) -> Result<i64> {
        let row = sqlx::query(&ph(
            "SELECT COUNT(*) AS cnt FROM api_keys \
             WHERE customer_id = ? \
             AND api_key_status_id = (SELECT id FROM api_key_status_lk WHERE code = 'active') \
             AND is_deleted = ?",
            self.is_postgres,
        ))
        .bind(customer_id)
        .bind(false)
        .fetch_one(&self.pool)
        .await?;
        row.try_get::<i64, _>("cnt")
            .or_else(|_| row.try_get::<i32, _>("cnt").map(|v| v as i64))
            .map_err(|e| anyhow!("count: {e}"))
    }

    async fn find_api_key_by_hash(&self, hash: &str) -> Result<Option<ApiKeyRecord>> {
        let sql = format!(
            "{} WHERE k.key_hash = ? AND k.is_deleted = ?",
            select_api_key_sql(self.is_postgres)
        );
        let row = sqlx::query(&ph(&sql, self.is_postgres))
            .bind(hash)
            .bind(false)
            .fetch_optional(&self.pool)
            .await?;
        row.map(|r| row_to_api_key(r, self.is_postgres)).transpose()
    }

    async fn revoke_api_key_by_urn(&self, customer_id: i64, urn: Uuid) -> Result<bool> {
        // Only revoke keys that are currently active. If the key is
        // already revoked (status != active), rows_affected == 0 and we
        // signal that to the caller (maps to 409).
        let now = Utc::now().to_rfc3339();
        let res = sqlx::query(&ph(
            "UPDATE api_keys SET \
             api_key_status_id = (SELECT id FROM api_key_status_lk WHERE code = 'revoked'), \
             revoked_at = ?@, is_deleted = ?, updated_at = ?@ \
             WHERE urn = ? AND customer_id = ? \
             AND api_key_status_id = (SELECT id FROM api_key_status_lk WHERE code = 'active')",
            self.is_postgres,
        ))
        .bind(&now)
        .bind(true)
        .bind(&now)
        .bind(urn.to_string())
        .bind(customer_id)
        .execute(&self.pool)
        .await?;
        Ok(res.rows_affected() > 0)
    }

    async fn revoke_all_for_customer(&self, customer_id: i64) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        sqlx::query(&ph(
            "UPDATE api_keys SET \
             api_key_status_id = (SELECT id FROM api_key_status_lk WHERE code = 'revoked'), \
             revoked_at = ?@, is_deleted = ?, updated_at = ?@ \
             WHERE customer_id = ? \
             AND api_key_status_id = (SELECT id FROM api_key_status_lk WHERE code = 'active')",
            self.is_postgres,
        ))
        .bind(&now)
        .bind(true)
        .bind(&now)
        .bind(customer_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn touch_last_used(&self, key_id: i64) -> Result<()> {
        sqlx::query(&ph(
            "UPDATE api_keys SET last_used_at = ?@ WHERE id = ?",
            self.is_postgres,
        ))
        .bind(Utc::now().to_rfc3339())
        .bind(key_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}

fn parse_uuid(s: &str) -> Result<Uuid> {
    Uuid::parse_str(s).map_err(|e| anyhow!("uuid parse: {e}"))
}

// sqlx::Any doesn't impl Decode<Any> for chrono types, so timestamps are
// read as String. SELECTs cast TIMESTAMPTZ → TEXT in postgres queries.
// For sqlite the column is already TEXT.
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
        billing_tier_id: row.try_get::<i32, _>("billing_tier_id")?,
        tier_code: row.try_get("tier_code")?,
        stripe_customer_id: row
            .try_get::<Option<String>, _>("stripe_customer_id")
            .ok()
            .flatten(),
        stripe_subscription_id: row
            .try_get::<Option<String>, _>("stripe_subscription_id")
            .ok()
            .flatten(),
        subscription_status_id: row.try_get::<i32, _>("subscription_status_id")?,
        subscription_status_code: row.try_get("subscription_status_code")?,
        subscription_type_id: row.try_get::<i32, _>("subscription_type_id")?,
        subscription_type_code: row.try_get("subscription_type_code")?,
        is_deleted: row
            .try_get::<bool, _>("is_deleted")
            .or_else(|_| row.try_get::<i32, _>("is_deleted").map(|v| v != 0))
            .unwrap_or(false),
        created_at: read_dt(&row, "created_at")?,
        updated_at: read_dt(&row, "updated_at")?,
    })
}

fn row_to_api_key(row: sqlx::any::AnyRow, is_postgres: bool) -> Result<ApiKeyRecord> {
    let urn: String = row.try_get("urn")?;
    // Postgres stores scopes as TEXT[] which sqlx::Any reads as a String
    // with format "{read,write}". Sqlite stores it as JSON text '["read","write"]'.
    let scopes = parse_scopes(&row, is_postgres)?;
    Ok(ApiKeyRecord {
        id: row.try_get::<i64, _>("id")?,
        urn: parse_uuid(&urn)?,
        customer_id: row.try_get::<i64, _>("customer_id")?,
        name: row.try_get("name")?,
        key_hash: row.try_get("key_hash")?,
        key_prefix: row.try_get("key_prefix")?,
        api_key_status_id: row.try_get::<i32, _>("api_key_status_id")?,
        status_code: row.try_get("status_code")?,
        scopes,
        expires_at: read_dt_opt(&row, "expires_at")?,
        revoked_at: read_dt_opt(&row, "revoked_at")?,
        is_deleted: row
            .try_get::<bool, _>("is_deleted")
            .or_else(|_| row.try_get::<i32, _>("is_deleted").map(|v| v != 0))
            .unwrap_or(false),
        created_at: read_dt(&row, "created_at")?,
        updated_at: read_dt(&row, "updated_at")?,
        last_used_at: read_dt_opt(&row, "last_used_at")?,
    })
}

/// Parse scopes from the row. Postgres TEXT[] comes back as a string like
/// `{read,write}` via sqlx::Any. Sqlite stores JSON `["read","write"]`.
fn parse_scopes(row: &sqlx::any::AnyRow, is_postgres: bool) -> Result<Vec<String>> {
    let raw: String = row.try_get::<String, _>("scopes").unwrap_or_else(|_| {
        if is_postgres {
            "{}".to_string()
        } else {
            "[]".to_string()
        }
    });
    if is_postgres {
        // Parse postgres array literal: {read,write} → ["read", "write"]
        let trimmed = raw.trim_start_matches('{').trim_end_matches('}');
        if trimmed.is_empty() {
            Ok(vec![])
        } else {
            Ok(trimmed.split(',').map(|s| s.trim().to_string()).collect())
        }
    } else {
        // Parse JSON array
        serde_json::from_str(&raw).map_err(|e| anyhow!("parse scopes: {e}"))
    }
}
