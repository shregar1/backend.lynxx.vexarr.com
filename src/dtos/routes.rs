use serde::Deserialize;
use uuid::Uuid;

use crate::abstractions::IRequestDto;

#[derive(Debug, Clone, Deserialize)]
pub struct CreateRoute {
    pub paths: Vec<String>,
    #[serde(default)]
    pub methods: Vec<String>,
    #[serde(default)]
    pub hosts: Vec<String>,
    pub service_id: Uuid,
    #[serde(default)]
    pub strip_path: bool,
}

impl IRequestDto for CreateRoute {}

#[derive(Debug, Clone, Deserialize)]
pub struct PatchRoute {
    pub paths: Option<Vec<String>>,
    pub methods: Option<Vec<String>>,
    pub hosts: Option<Vec<String>>,
    pub service_id: Option<Uuid>,
    pub strip_path: Option<bool>,
}

impl IRequestDto for PatchRoute {}
