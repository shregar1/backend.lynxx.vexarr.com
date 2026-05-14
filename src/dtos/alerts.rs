use serde::Deserialize;

use crate::abstractions::IRequestDto;

fn default_operator() -> String {
    "gt".into()
}

fn default_window() -> i32 {
    300
}

fn default_cooldown() -> i32 {
    600
}

#[derive(Debug, Clone, Deserialize)]
pub struct CreateAlertRuleBody {
    pub name: String,
    pub metric: String,
    #[serde(default = "default_operator")]
    pub operator: String,
    pub threshold: f64,
    #[serde(default = "default_window")]
    pub window_seconds: i32,
    #[serde(default = "default_cooldown")]
    pub cooldown_seconds: i32,
    #[serde(default)]
    pub channels: serde_json::Value,
}

impl IRequestDto for CreateAlertRuleBody {}

#[derive(Debug, Clone, Deserialize)]
pub struct PatchAlertRuleBody {
    pub name: Option<String>,
    pub metric: Option<String>,
    pub operator: Option<String>,
    pub threshold: Option<f64>,
    pub window_seconds: Option<i32>,
    pub cooldown_seconds: Option<i32>,
    pub channels: Option<serde_json::Value>,
    pub enabled: Option<bool>,
}

impl IRequestDto for PatchAlertRuleBody {}
