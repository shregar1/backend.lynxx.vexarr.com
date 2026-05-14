use serde::Deserialize;
use uuid::Uuid;

use crate::abstractions::IRequestDto;

fn default_rps() -> i32 {
    100
}

fn default_duration() -> i32 {
    60
}

fn default_window() -> i32 {
    60
}

#[derive(Debug, Clone, Deserialize)]
pub struct RunSimulationBody {
    #[serde(default)]
    pub consumer_id: Option<Uuid>,
    #[serde(default = "default_rps")]
    pub requests_per_second: i32,
    #[serde(default = "default_duration")]
    pub duration_seconds: i32,
    pub limit: i32,
    #[serde(default = "default_window")]
    pub window_seconds: i32,
    #[serde(default)]
    pub burst: i32,
}

impl IRequestDto for RunSimulationBody {}
