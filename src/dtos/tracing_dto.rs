use serde::Deserialize;
use uuid::Uuid;

use crate::abstractions::IRequestDto;

#[derive(Debug, Clone, Deserialize)]
pub struct IngestTraceBody {
    pub trace_id: String,
    #[serde(default)]
    pub route_id: Option<Uuid>,
    pub method: String,
    pub path: String,
    pub status: i32,
    pub duration_ms: i64,
    #[serde(default)]
    pub span_count: i32,
    #[serde(default)]
    pub has_errors: bool,
    #[serde(default)]
    pub metadata: Option<serde_json::Value>,
    #[serde(default)]
    pub spans: Vec<IngestSpanBody>,
}

impl IRequestDto for IngestTraceBody {}

#[derive(Debug, Clone, Deserialize)]
pub struct IngestSpanBody {
    pub span_id: String,
    #[serde(default)]
    pub parent_span_id: Option<String>,
    pub operation: String,
    pub service_name: String,
    #[serde(default = "default_status")]
    pub status: String,
    pub duration_ms: i64,
    #[serde(default)]
    pub tags: Option<serde_json::Value>,
    #[serde(default)]
    pub logs: Option<serde_json::Value>,
}

fn default_status() -> String {
    "ok".into()
}

impl IRequestDto for IngestSpanBody {}
