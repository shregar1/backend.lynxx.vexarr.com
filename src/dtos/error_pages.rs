use serde::Deserialize;

use crate::abstractions::IRequestDto;

fn default_content_type() -> String {
    "text/html".into()
}

#[derive(Debug, Clone, Deserialize)]
pub struct CreateErrorPageBody {
    pub status_code: i32,
    #[serde(default = "default_content_type")]
    pub content_type: String,
    pub body: String,
}

impl IRequestDto for CreateErrorPageBody {}

#[derive(Debug, Clone, Deserialize)]
pub struct PatchErrorPageBody {
    pub status_code: Option<i32>,
    pub content_type: Option<String>,
    pub body: Option<String>,
    pub enabled: Option<bool>,
}

impl IRequestDto for PatchErrorPageBody {}
