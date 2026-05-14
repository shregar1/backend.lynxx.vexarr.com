use serde::Deserialize;
use uuid::Uuid;

use crate::abstractions::IRequestDto;

fn default_retries() -> u32 {
    3
}

fn default_timeout() -> u64 {
    5000
}

#[derive(Debug, Clone, Deserialize)]
pub struct CreateService {
    pub name: String,
    #[serde(default)]
    pub url: String,
    #[serde(default = "default_retries")]
    pub retries: u32,
    #[serde(default = "default_timeout")]
    pub connect_timeout_ms: u64,
    #[serde(default)]
    pub lb_strategy: Option<String>,
    #[serde(default)]
    pub service_type: Option<String>,
    #[serde(default)]
    pub document_root: Option<String>,
    #[serde(default)]
    pub index_file: Option<String>,
    #[serde(default)]
    pub spa_fallback: Option<bool>,
    #[serde(default)]
    pub project_id: Option<Uuid>,
    #[serde(default)]
    pub domain: Option<String>,
    #[serde(default)]
    pub subdomain: Option<String>,
}

impl IRequestDto for CreateService {}

#[derive(Debug, Clone, Deserialize)]
pub struct PatchService {
    pub name: Option<String>,
    pub url: Option<String>,
    pub retries: Option<u32>,
    pub connect_timeout_ms: Option<u64>,
    pub lb_strategy: Option<String>,
    pub service_type: Option<String>,
    pub document_root: Option<String>,
    pub index_file: Option<String>,
    pub spa_fallback: Option<bool>,
    pub project_id: Option<Uuid>,
    pub domain: Option<String>,
    pub subdomain: Option<String>,
}

impl IRequestDto for PatchService {}
