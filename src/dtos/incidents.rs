use chrono::{DateTime, Utc};
use serde::Deserialize;

use crate::abstractions::IRequestDto;

fn default_severity() -> String {
    "minor".into()
}

fn default_incident_status() -> String {
    "investigating".into()
}

fn default_update_status() -> String {
    "update".into()
}

#[derive(Debug, Clone, Deserialize)]
pub struct CreateIncidentBody {
    pub title: String,
    #[serde(default = "default_severity")]
    pub severity: String,
    #[serde(default = "default_incident_status")]
    pub status: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub started_at: Option<DateTime<Utc>>,
}

impl IRequestDto for CreateIncidentBody {}

#[derive(Debug, Clone, Deserialize)]
pub struct PatchIncidentBody {
    pub title: Option<String>,
    pub severity: Option<String>,
    pub status: Option<String>,
    pub description: Option<String>,
    pub resolved_at: Option<Option<DateTime<Utc>>>,
}

impl IRequestDto for PatchIncidentBody {}

#[derive(Debug, Clone, Deserialize)]
pub struct CreateIncidentUpdateBody {
    #[serde(default = "default_update_status")]
    pub status: String,
    pub message: String,
}

impl IRequestDto for CreateIncidentUpdateBody {}
