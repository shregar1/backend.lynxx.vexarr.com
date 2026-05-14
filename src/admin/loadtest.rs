use crate::admin::routes::AdminState;
use crate::error::GatewayError;
use crate::models::LoadTest;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use chrono::Utc;
use serde::Deserialize;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// GET /services/:id/load-tests — list load tests for a service
// ---------------------------------------------------------------------------

pub async fn list_load_tests(
    State(state): State<AdminState>,
    Path(service_id): Path<Uuid>,
) -> Result<Json<Vec<LoadTest>>, GatewayError> {
    let tests = state.repo.list_load_tests(&service_id).await?;
    Ok(Json(tests))
}

// ---------------------------------------------------------------------------
// POST /services/:id/load-tests — create and start a load test
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct CreateLoadTestBody {
    pub name: String,
    #[serde(default = "default_rps")]
    pub target_rps: i32,
    #[serde(default = "default_duration")]
    pub duration_seconds: i32,
    #[serde(default = "default_users")]
    pub concurrent_users: i32,
    #[serde(default = "default_method")]
    pub method: String,
    #[serde(default = "default_path")]
    pub path: String,
    #[serde(default)]
    pub headers: Option<serde_json::Value>,
    #[serde(default)]
    pub body: Option<String>,
}

fn default_rps() -> i32 {
    100
}
fn default_duration() -> i32 {
    30
}
fn default_users() -> i32 {
    10
}
fn default_method() -> String {
    "GET".into()
}
fn default_path() -> String {
    "/".into()
}

pub async fn create_load_test(
    State(state): State<AdminState>,
    Path(service_id): Path<Uuid>,
    Json(body): Json<CreateLoadTestBody>,
) -> Result<(StatusCode, Json<LoadTest>), GatewayError> {
    let now = Utc::now();
    let test = LoadTest {
        id: Uuid::new_v4(),
        service_id,
        name: body.name,
        target_rps: body.target_rps,
        duration_seconds: body.duration_seconds,
        concurrent_users: body.concurrent_users,
        method: body.method,
        path: body.path,
        headers: body.headers,
        body: body.body,
        status: "pending".into(),
        result: None,
        started_at: None,
        completed_at: None,
        created_at: now,
    };
    state.repo.create_load_test(&test).await?;
    Ok((StatusCode::CREATED, Json(test)))
}

// ---------------------------------------------------------------------------
// GET /load-tests/:id — get a single load test
// ---------------------------------------------------------------------------

pub async fn get_load_test(
    State(state): State<AdminState>,
    Path(id): Path<Uuid>,
) -> Result<Json<LoadTest>, GatewayError> {
    let test = state
        .repo
        .get_load_test(&id)
        .await?
        .ok_or(GatewayError::NotFound("load test not found".into()))?;
    Ok(Json(test))
}

// ---------------------------------------------------------------------------
// DELETE /load-tests/:id — delete a load test
// ---------------------------------------------------------------------------

pub async fn delete_load_test(
    State(state): State<AdminState>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, GatewayError> {
    if state.repo.delete_load_test(&id).await? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(GatewayError::NotFound("load test not found".into()))
    }
}
