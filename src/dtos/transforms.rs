use serde::Deserialize;
use uuid::Uuid;

use crate::abstractions::IRequestDto;

fn default_phase() -> String {
    "request".into()
}

fn default_enabled() -> bool {
    true
}

#[derive(Debug, Clone, Deserialize)]
pub struct CreateTransformBody {
    pub name: String,
    #[serde(default)]
    pub route_id: Option<Uuid>,
    #[serde(default = "default_phase")]
    pub phase: String,
    pub action: String,
    pub target: String,
    #[serde(default)]
    pub value: String,
    #[serde(default)]
    pub condition: Option<String>,
    #[serde(default)]
    pub priority: i32,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
}

impl IRequestDto for CreateTransformBody {}

#[derive(Debug, Clone, Deserialize)]
pub struct PatchTransformBody {
    pub name: Option<String>,
    pub route_id: Option<Option<Uuid>>,
    pub phase: Option<String>,
    pub action: Option<String>,
    pub target: Option<String>,
    pub value: Option<String>,
    pub condition: Option<Option<String>>,
    pub priority: Option<i32>,
    pub enabled: Option<bool>,
}

impl IRequestDto for PatchTransformBody {}
