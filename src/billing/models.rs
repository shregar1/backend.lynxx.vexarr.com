use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// `id` is the internal primary key — never returned to the outside world.
/// `urn` is the external identifier used in URL paths and JSON payloads.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Customer {
    pub id: Uuid,
    pub urn: Uuid,
    pub email: String,
    pub password_hash: String,
    pub name: Option<String>,
    pub company: Option<String>,
    pub tier: String,
    pub stripe_customer_id: Option<String>,
    pub stripe_subscription_id: Option<String>,
    pub subscription_status: String,
    pub is_deleted: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiKeyRecord {
    pub id: Uuid,
    pub urn: Uuid,
    pub customer_id: Uuid,
    pub name: String,
    pub key_hash: String,
    pub key_prefix: String,
    pub status: String,
    pub expires_at: Option<DateTime<Utc>>,
    pub is_deleted: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub last_used_at: Option<DateTime<Utc>>,
}
