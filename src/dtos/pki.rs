use chrono::{DateTime, Utc};
use serde::Deserialize;
use uuid::Uuid;

use crate::abstractions::IRequestDto;

fn default_issuer() -> String {
    "unknown".into()
}

fn default_auto_renew() -> bool {
    true
}

fn default_cert_status() -> String {
    "active".into()
}

#[derive(Debug, Clone, Deserialize)]
pub struct CreateCertificateBody {
    #[serde(default)]
    pub service_id: Option<Uuid>,
    pub domain: String,
    #[serde(default = "default_issuer")]
    pub issuer: String,
    #[serde(default)]
    pub serial_number: String,
    pub not_before: DateTime<Utc>,
    pub not_after: DateTime<Utc>,
    #[serde(default)]
    pub fingerprint_sha256: String,
    #[serde(default = "default_auto_renew")]
    pub auto_renew: bool,
    #[serde(default = "default_cert_status")]
    pub status: String,
}

impl IRequestDto for CreateCertificateBody {}
