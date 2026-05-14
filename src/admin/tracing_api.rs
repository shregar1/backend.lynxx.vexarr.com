use crate::admin::routes::AdminState;
use crate::error::GatewayError;
use crate::models::{SpanRecord, TraceRecord};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Query params
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct TraceQuery {
    #[serde(default = "default_limit")]
    pub limit: u32,
}

fn default_limit() -> u32 {
    100
}

// ---------------------------------------------------------------------------
// GET /services/:id/traces — list traces for a service
// ---------------------------------------------------------------------------

pub async fn list_traces(
    State(state): State<AdminState>,
    Path(service_id): Path<Uuid>,
    Query(q): Query<TraceQuery>,
) -> Result<Json<Vec<TraceRecord>>, GatewayError> {
    let traces = state.repo.list_traces(&service_id, q.limit).await?;
    Ok(Json(traces))
}

// ---------------------------------------------------------------------------
// GET /traces/:id — get a single trace
// ---------------------------------------------------------------------------

pub async fn get_trace(
    State(state): State<AdminState>,
    Path(id): Path<Uuid>,
) -> Result<Json<TraceRecord>, GatewayError> {
    let trace = state
        .repo
        .get_trace(&id)
        .await?
        .ok_or(GatewayError::NotFound("trace not found".into()))?;
    Ok(Json(trace))
}

// ---------------------------------------------------------------------------
// GET /traces/:trace_id/spans — list spans for a trace
// ---------------------------------------------------------------------------

pub async fn list_spans(
    State(state): State<AdminState>,
    Path(trace_id): Path<String>,
) -> Result<Json<Vec<SpanRecord>>, GatewayError> {
    let spans = state.repo.list_spans(&trace_id).await?;
    Ok(Json(spans))
}

// ---------------------------------------------------------------------------
// POST /services/:id/traces — ingest a new trace
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
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

#[derive(Deserialize)]
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

pub async fn ingest_trace(
    State(state): State<AdminState>,
    Path(service_id): Path<Uuid>,
    Json(body): Json<IngestTraceBody>,
) -> Result<(StatusCode, Json<TraceRecord>), GatewayError> {
    let now = chrono::Utc::now();
    let trace = TraceRecord {
        id: Uuid::new_v4(),
        trace_id: body.trace_id.clone(),
        service_id,
        route_id: body.route_id,
        method: body.method,
        path: body.path,
        status: body.status,
        duration_ms: body.duration_ms,
        span_count: body.span_count.max(body.spans.len() as i32),
        has_errors: body.has_errors,
        metadata: body.metadata,
        started_at: now,
    };
    state.repo.create_trace(&trace).await?;

    for s in body.spans {
        let span = SpanRecord {
            id: Uuid::new_v4(),
            trace_id: body.trace_id.clone(),
            span_id: s.span_id,
            parent_span_id: s.parent_span_id,
            operation: s.operation,
            service_name: s.service_name,
            status: s.status,
            duration_ms: s.duration_ms,
            tags: s.tags,
            logs: s.logs,
            started_at: now,
        };
        state.repo.create_span(&span).await?;
    }

    Ok((StatusCode::CREATED, Json(trace)))
}
