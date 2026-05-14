use serde::Deserialize;
use uuid::Uuid;

use crate::abstractions::IRequestDto;

fn default_method() -> String {
    "GET".into()
}

fn default_path() -> String {
    "/".into()
}

fn default_interval() -> i32 {
    60
}

fn default_timeout() -> i32 {
    5000
}

fn default_expected() -> i32 {
    200
}

#[derive(Debug, Clone, Deserialize)]
pub struct CreateProbeBody {
    pub name: String,
    #[serde(default)]
    pub route_id: Option<Uuid>,
    #[serde(default = "default_method")]
    pub method: String,
    #[serde(default = "default_path")]
    pub path: String,
    #[serde(default = "default_interval")]
    pub interval_seconds: i32,
    #[serde(default = "default_timeout")]
    pub timeout_ms: i32,
    #[serde(default = "default_expected")]
    pub expected_status: i32,
}

impl IRequestDto for CreateProbeBody {}

#[derive(Debug, Clone, Deserialize)]
pub struct PatchProbeBody {
    pub enabled: Option<bool>,
    pub name: Option<String>,
    pub method: Option<String>,
    pub path: Option<String>,
    pub interval_seconds: Option<i32>,
    pub timeout_ms: Option<i32>,
    pub expected_status: Option<i32>,
}

impl IRequestDto for PatchProbeBody {}
