use serde::Deserialize;

use crate::abstractions::IRequestDto;

fn default_timeout_ms() -> u64 {
    30000
}

#[derive(Debug, Clone, Deserialize)]
pub struct DrainNodeBody {
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,
    #[serde(default)]
    pub graceful: bool,
}

impl IRequestDto for DrainNodeBody {}
