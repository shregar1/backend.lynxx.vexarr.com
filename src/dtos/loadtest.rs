use serde::Deserialize;

use crate::abstractions::IRequestDto;

fn default_rps() -> i32 {
    100
}

fn default_duration() -> i32 {
    30
}

fn default_users() -> i32 {
    10
}

fn default_method() -> String {
    "GET".into()
}

fn default_path() -> String {
    "/".into()
}

#[derive(Debug, Clone, Deserialize)]
pub struct CreateLoadTestBody {
    pub name: String,
    #[serde(default = "default_rps")]
    pub target_rps: i32,
    #[serde(default = "default_duration")]
    pub duration_seconds: i32,
    #[serde(default = "default_users")]
    pub concurrent_users: i32,
    #[serde(default = "default_method")]
    pub method: String,
    #[serde(default = "default_path")]
    pub path: String,
    #[serde(default)]
    pub headers: Option<serde_json::Value>,
    #[serde(default)]
    pub body: Option<String>,
}

impl IRequestDto for CreateLoadTestBody {}
