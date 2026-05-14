use serde::Deserialize;

use crate::abstractions::IRequestDto;

#[derive(Debug, Clone, Deserialize)]
pub struct CreateProject {
    pub name: String,
    #[serde(default)]
    pub description: String,
}

impl IRequestDto for CreateProject {}

#[derive(Debug, Clone, Deserialize)]
pub struct PatchProject {
    pub name: Option<String>,
    pub description: Option<String>,
}

impl IRequestDto for PatchProject {}
