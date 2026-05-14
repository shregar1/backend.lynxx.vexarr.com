use chrono::{DateTime, Utc};
use serde::Deserialize;

use crate::abstractions::IRequestDto;

fn default_status() -> String {
    "active".into()
}

#[derive(Debug, Clone, Deserialize)]
pub struct CreateVersionBody {
    pub version: String,
    pub base_path: String,
    #[serde(default)]
    pub upstream_path: String,
    #[serde(default = "default_status")]
    pub status: String,
    #[serde(default)]
    pub deprecated_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub sunset_at: Option<DateTime<Utc>>,
}

impl IRequestDto for CreateVersionBody {}

#[derive(Debug, Clone, Deserialize)]
pub struct PatchVersionBody {
    pub version: Option<String>,
    pub base_path: Option<String>,
    pub upstream_path: Option<String>,
    pub status: Option<String>,
    pub deprecated_at: Option<Option<DateTime<Utc>>>,
    pub sunset_at: Option<Option<DateTime<Utc>>>,
}

impl IRequestDto for PatchVersionBody {}
