use serde::Deserialize;

use crate::abstractions::IRequestDto;

fn default_weight() -> u32 {
    1
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Deserialize)]
pub struct CreateTarget {
    pub url: String,
    #[serde(default = "default_weight")]
    pub weight: u32,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub zone: Option<String>,
}

impl IRequestDto for CreateTarget {}

#[derive(Debug, Clone, Deserialize)]
pub struct UpdateTarget {
    pub url: Option<String>,
    pub weight: Option<u32>,
    pub enabled: Option<bool>,
    pub zone: Option<String>,
}

impl IRequestDto for UpdateTarget {}
