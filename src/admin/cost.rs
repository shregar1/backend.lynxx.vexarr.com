use crate::admin::routes::AdminState;
use crate::error::GatewayError;
use crate::models::CostRecord;
use axum::extract::{Path, Query, State};
use axum::Json;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// GET /services/:id/cost — list cost records for a service
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct CostQuery {
    #[serde(default = "default_from")]
    pub from: DateTime<Utc>,
    #[serde(default = "default_to")]
    pub to: DateTime<Utc>,
}

fn default_from() -> DateTime<Utc> {
    Utc::now() - chrono::Duration::days(30)
}

fn default_to() -> DateTime<Utc> {
    Utc::now()
}

pub async fn list_cost_records(
    State(state): State<AdminState>,
    Path(service_id): Path<Uuid>,
    Query(q): Query<CostQuery>,
) -> Result<Json<Vec<CostRecord>>, GatewayError> {
    let records = state
        .repo
        .list_cost_records(&service_id, q.from, q.to)
        .await?;
    Ok(Json(records))
}

// ---------------------------------------------------------------------------
// GET /services/:id/cost/summary — aggregated cost summary
// ---------------------------------------------------------------------------

#[derive(Serialize)]
pub struct CostSummary {
    pub service_id: Uuid,
    pub from: DateTime<Utc>,
    pub to: DateTime<Utc>,
    pub total_compute_cost: f64,
    pub total_bandwidth_cost: f64,
    pub total_storage_cost: f64,
    pub total_cost: f64,
    pub total_requests: i64,
    pub total_bytes_transferred: i64,
    pub currency: String,
    pub record_count: usize,
}

pub async fn cost_summary(
    State(state): State<AdminState>,
    Path(service_id): Path<Uuid>,
    Query(q): Query<CostQuery>,
) -> Result<Json<CostSummary>, GatewayError> {
    let records = state
        .repo
        .list_cost_records(&service_id, q.from, q.to)
        .await?;

    let mut total_compute = 0.0f64;
    let mut total_bandwidth = 0.0f64;
    let mut total_storage = 0.0f64;
    let mut total_cost = 0.0f64;
    let mut total_requests = 0i64;
    let mut total_bytes = 0i64;
    let mut currency = "USD".to_string();

    for r in &records {
        total_compute += r.compute_cost;
        total_bandwidth += r.bandwidth_cost;
        total_storage += r.storage_cost;
        total_cost += r.total_cost;
        total_requests += r.requests;
        total_bytes += r.bytes_transferred;
        currency = r.currency.clone();
    }

    Ok(Json(CostSummary {
        service_id,
        from: q.from,
        to: q.to,
        total_compute_cost: total_compute,
        total_bandwidth_cost: total_bandwidth,
        total_storage_cost: total_storage,
        total_cost,
        total_requests,
        total_bytes_transferred: total_bytes,
        currency,
        record_count: records.len(),
    }))
}
