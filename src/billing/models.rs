use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// `id` is the internal primary key — never returned to the outside world.
/// `urn` is the external identifier used in URL paths and JSON payloads.
///
/// FK columns (`billing_tier_id`, `subscription_status_id`,
/// `subscription_type_id`) are the authoritative enum values.  The
/// corresponding `*_code` fields are resolved via JOIN against the lookup
/// tables at read time — they exist for convenience in business logic and
/// API responses but are NEVER written back to the database.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Customer {
    pub id: i64,
    pub urn: Uuid,
    pub email: String,
    pub password_hash: String,
    pub name: Option<String>,
    pub company: Option<String>,
    // FK → billing_tier_lk
    pub billing_tier_id: i32,
    /// Resolved from `billing_tier_lk.code` via JOIN (read-only).
    pub tier_code: String,
    pub stripe_customer_id: Option<String>,
    pub stripe_subscription_id: Option<String>,
    // FK → subscription_status_lk
    pub subscription_status_id: i32,
    /// Resolved from `subscription_status_lk.code` via JOIN (read-only).
    pub subscription_status_code: String,
    // FK → subscription_type_lk
    pub subscription_type_id: i32,
    /// Resolved from `subscription_type_lk.code` via JOIN (read-only).
    pub subscription_type_code: String,
    pub is_deleted: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiKeyRecord {
    pub id: i64,
    pub urn: Uuid,
    pub customer_id: i64,
    pub name: String,
    pub key_hash: String,
    pub key_prefix: String,
    // FK → api_key_status_lk
    pub api_key_status_id: i32,
    /// Resolved from `api_key_status_lk.code` via JOIN (read-only).
    pub status_code: String,
    /// Permitted operations (e.g. ["read", "write", "admin"]).
    pub scopes: Vec<String>,
    pub expires_at: Option<DateTime<Utc>>,
    /// Timestamp of revocation (None while active).
    pub revoked_at: Option<DateTime<Utc>>,
    pub is_deleted: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub last_used_at: Option<DateTime<Utc>>,
}

/// Insert payload — no `id` (assigned by the DB) and no `created_at`/
/// `updated_at` (defaulted in SQL). The repo returns the new `id`.
///
/// Enum fields use the lookup-table **code** string (`"free"`, `"active"`,
/// etc.). The repo resolves them to FK IDs via subquery at INSERT time.
#[derive(Debug, Clone)]
pub struct NewCustomer {
    pub urn: Uuid,
    pub email: String,
    pub password_hash: String,
    pub name: Option<String>,
    pub company: Option<String>,
    /// billing_tier_lk.code — e.g. "free", "pro", "business", "enterprise"
    pub tier_code: String,
    pub stripe_customer_id: Option<String>,
    pub stripe_subscription_id: Option<String>,
    /// subscription_status_lk.code — e.g. "active", "past_due", "canceled"
    pub subscription_status_code: String,
    /// subscription_type_lk.code — e.g. "free", "trial", "paid", "comp", "internal"
    pub subscription_type_code: String,
}

#[derive(Debug, Clone)]
pub struct NewApiKey {
    pub urn: Uuid,
    pub customer_id: i64,
    pub name: String,
    pub key_hash: String,
    pub key_prefix: String,
    /// api_key_status_lk.code — e.g. "active", "revoked", "expired"
    pub status_code: String,
    /// Permitted operations (e.g. ["read", "write"]).
    pub scopes: Vec<String>,
    pub expires_at: Option<DateTime<Utc>>,
}
