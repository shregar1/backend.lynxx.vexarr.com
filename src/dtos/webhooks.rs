use serde::Deserialize;

use crate::abstractions::IRequestDto;

fn default_events() -> serde_json::Value {
    serde_json::json!(["*"])
}

fn default_enabled() -> bool {
    true
}

fn default_retry_count() -> i32 {
    3
}

fn default_timeout_ms() -> i32 {
    5000
}

#[derive(Debug, Clone, Deserialize)]
pub struct CreateWebhookBody {
    pub name: String,
    pub url: String,
    #[serde(default = "default_events")]
    pub events: serde_json::Value,
    #[serde(default)]
    pub secret: Option<String>,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(default = "default_retry_count")]
    pub retry_count: i32,
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: i32,
}

impl IRequestDto for CreateWebhookBody {}

#[derive(Debug, Clone, Deserialize)]
pub struct PatchWebhookBody {
    pub name: Option<String>,
    pub url: Option<String>,
    pub events: Option<serde_json::Value>,
    pub secret: Option<Option<String>>,
    pub enabled: Option<bool>,
    pub retry_count: Option<i32>,
    pub timeout_ms: Option<i32>,
}

impl IRequestDto for PatchWebhookBody {}
