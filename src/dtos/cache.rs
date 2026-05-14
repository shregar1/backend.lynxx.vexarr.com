use serde::Deserialize;

use crate::abstractions::IRequestDto;

fn default_ttl() -> i32 {
    3600
}

#[derive(Debug, Clone, Deserialize)]
pub struct WarmCacheBody {
    pub urls: Vec<String>,
    #[serde(default = "default_ttl")]
    pub ttl_seconds: i32,
}

impl IRequestDto for WarmCacheBody {}

#[derive(Debug, Clone, Deserialize)]
pub struct PurgeCacheBody {
    #[serde(default)]
    pub keys: Option<Vec<String>>,
}

impl IRequestDto for PurgeCacheBody {}
