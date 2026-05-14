use crate::admin::routes::AdminState;
use crate::error::GatewayError;
use crate::models::TransformRule;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use chrono::Utc;
use serde::Deserialize;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// GET /services/:id/transforms — list transform rules for a service
// ---------------------------------------------------------------------------

pub async fn list_transforms(
    State(state): State<AdminState>,
    Path(service_id): Path<Uuid>,
) -> Result<Json<Vec<TransformRule>>, GatewayError> {
    let rules = state.repo.list_transform_rules(&service_id).await?;
    Ok(Json(rules))
}

// ---------------------------------------------------------------------------
// POST /services/:id/transforms — create a new transform rule
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct CreateTransformBody {
    pub name: String,
    #[serde(default)]
    pub route_id: Option<Uuid>,
    #[serde(default = "default_phase")]
    pub phase: String,
    pub action: String,
    pub target: String,
    #[serde(default)]
    pub value: String,
    #[serde(default)]
    pub condition: Option<String>,
    #[serde(default)]
    pub priority: i32,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
}

fn default_phase() -> String {
    "request".into()
}
fn default_enabled() -> bool {
    true
}

pub async fn create_transform(
    State(state): State<AdminState>,
    Path(service_id): Path<Uuid>,
    Json(body): Json<CreateTransformBody>,
) -> Result<(StatusCode, Json<TransformRule>), GatewayError> {
    let now = Utc::now();
    let rule = TransformRule {
        id: Uuid::new_v4(),
        service_id,
        route_id: body.route_id,
        name: body.name,
        phase: body.phase,
        action: body.action,
        target: body.target,
        value: body.value,
        condition: body.condition,
        priority: body.priority,
        enabled: body.enabled,
        created_at: now,
        updated_at: now,
    };
    state.repo.create_transform_rule(&rule).await?;
    Ok((StatusCode::CREATED, Json(rule)))
}

// ---------------------------------------------------------------------------
// PATCH /transforms/:id — update an existing transform rule
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct PatchTransformBody {
    pub name: Option<String>,
    pub route_id: Option<Option<Uuid>>,
    pub phase: Option<String>,
    pub action: Option<String>,
    pub target: Option<String>,
    pub value: Option<String>,
    pub condition: Option<Option<String>>,
    pub priority: Option<i32>,
    pub enabled: Option<bool>,
}

pub async fn patch_transform(
    State(state): State<AdminState>,
    Path(id): Path<Uuid>,
    Json(body): Json<PatchTransformBody>,
) -> Result<Json<TransformRule>, GatewayError> {
    let mut rule = state
        .repo
        .get_transform_rule(&id)
        .await?
        .ok_or(GatewayError::NotFound("transform rule not found".into()))?;

    if let Some(v) = body.name {
        rule.name = v;
    }
    if let Some(v) = body.route_id {
        rule.route_id = v;
    }
    if let Some(v) = body.phase {
        rule.phase = v;
    }
    if let Some(v) = body.action {
        rule.action = v;
    }
    if let Some(v) = body.target {
        rule.target = v;
    }
    if let Some(v) = body.value {
        rule.value = v;
    }
    if let Some(v) = body.condition {
        rule.condition = v;
    }
    if let Some(v) = body.priority {
        rule.priority = v;
    }
    if let Some(v) = body.enabled {
        rule.enabled = v;
    }
    rule.updated_at = Utc::now();

    state.repo.update_transform_rule(&id, &rule).await?;
    Ok(Json(rule))
}

// ---------------------------------------------------------------------------
// DELETE /transforms/:id — delete a transform rule
// ---------------------------------------------------------------------------

pub async fn delete_transform(
    State(state): State<AdminState>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, GatewayError> {
    if state.repo.delete_transform_rule(&id).await? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(GatewayError::NotFound("transform rule not found".into()))
    }
}
