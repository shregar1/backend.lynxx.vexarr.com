use serde::Deserialize;
use uuid::Uuid;

use crate::abstractions::IRequestDto;

#[derive(Debug, Clone, Deserialize)]
pub struct InstallPluginBody {
    #[serde(default)]
    pub service_id: Option<Uuid>,
    #[serde(default)]
    pub route_id: Option<Uuid>,
    #[serde(default)]
    pub config: serde_json::Value,
}

impl IRequestDto for InstallPluginBody {}
