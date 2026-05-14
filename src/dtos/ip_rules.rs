use serde::Deserialize;
use uuid::Uuid;

use crate::abstractions::IRequestDto;

fn default_action() -> String {
    "deny".into()
}

#[derive(Debug, Clone, Deserialize)]
pub struct CreateIpRuleBody {
    pub cidr: String,
    #[serde(default = "default_action")]
    pub action: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub priority: i32,
    #[serde(default)]
    pub route_id: Option<Uuid>,
}

impl IRequestDto for CreateIpRuleBody {}

#[derive(Debug, Clone, Deserialize)]
pub struct PatchIpRuleBody {
    pub cidr: Option<String>,
    pub action: Option<String>,
    pub description: Option<String>,
    pub priority: Option<i32>,
    pub enabled: Option<bool>,
}

impl IRequestDto for PatchIpRuleBody {}

#[derive(Debug, Clone, Deserialize)]
pub struct BulkIpRulesBody {
    pub rules: Vec<CreateIpRuleBody>,
}

impl IRequestDto for BulkIpRulesBody {}
