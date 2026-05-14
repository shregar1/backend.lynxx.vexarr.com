use serde::Deserialize;
use uuid::Uuid;

use crate::abstractions::IRequestDto;

fn default_rate_limit() -> i32 {
    100
}

fn default_window() -> i32 {
    60
}

#[derive(Debug, Clone, Deserialize)]
pub struct CreateConsumerGroupBody {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default = "default_rate_limit")]
    pub rate_limit: i32,
    #[serde(default = "default_window")]
    pub rate_window_seconds: i32,
    #[serde(default)]
    pub burst_limit: i32,
}

impl IRequestDto for CreateConsumerGroupBody {}

#[derive(Debug, Clone, Deserialize)]
pub struct PatchConsumerGroupBody {
    pub name: Option<String>,
    pub description: Option<String>,
    pub rate_limit: Option<i32>,
    pub rate_window_seconds: Option<i32>,
    pub burst_limit: Option<i32>,
}

impl IRequestDto for PatchConsumerGroupBody {}

#[derive(Debug, Clone, Deserialize)]
pub struct AddMemberBody {
    pub consumer_id: Uuid,
}

impl IRequestDto for AddMemberBody {}
