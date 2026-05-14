use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::abstractions::{IRequestDto, IResponseDto};
use crate::auth::Role;

#[derive(Debug, Clone, Deserialize)]
pub struct CreateTokenInput {
    pub name: String,
    pub role: Role,
    pub expires_at: Option<DateTime<Utc>>,
}

impl IRequestDto for CreateTokenInput {}

#[derive(Debug, Clone, Deserialize)]
pub struct UpdateTokenInput {
    pub name: Option<String>,
    pub role: Option<Role>,
    /// `null` clears the expiry; field absence leaves it unchanged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<Option<DateTime<Utc>>>,
}

impl IRequestDto for UpdateTokenInput {}

#[derive(Debug, Clone, Serialize)]
pub struct CreatedTokenResponse {
    pub id: Uuid,
    pub name: String,
    pub role: Role,
    pub created_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
    pub last_used_at: Option<DateTime<Utc>>,
    /// Raw token. Returned ONCE on creation; never retrievable again.
    pub token: String,
}

impl IResponseDto for CreatedTokenResponse {}
