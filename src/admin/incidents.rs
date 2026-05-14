use crate::admin::routes::AdminState;
use crate::error::GatewayError;
use crate::models::{Incident, IncidentUpdate};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// GET /services/:id/incidents — list incidents for a service
// ---------------------------------------------------------------------------

pub async fn list_incidents(
    State(state): State<AdminState>,
    Path(service_id): Path<Uuid>,
) -> Result<Json<Vec<Incident>>, GatewayError> {
    let incidents = state.repo.list_incidents(&service_id).await?;
    Ok(Json(incidents))
}

// ---------------------------------------------------------------------------
// GET /incidents/:id — get a single incident
// ---------------------------------------------------------------------------

pub async fn get_incident(
    State(state): State<AdminState>,
    Path(id): Path<Uuid>,
) -> Result<Json<Incident>, GatewayError> {
    let incident = state
        .repo
        .get_incident(&id)
        .await?
        .ok_or(GatewayError::NotFound("incident not found".into()))?;
    Ok(Json(incident))
}

// ---------------------------------------------------------------------------
// POST /services/:id/incidents — create a new incident
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
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

fn default_severity() -> String {
    "minor".into()
}
fn default_incident_status() -> String {
    "investigating".into()
}

pub async fn create_incident(
    State(state): State<AdminState>,
    Path(service_id): Path<Uuid>,
    Json(body): Json<CreateIncidentBody>,
) -> Result<(StatusCode, Json<Incident>), GatewayError> {
    let now = Utc::now();
    let incident = Incident {
        id: Uuid::new_v4(),
        service_id,
        title: body.title,
        severity: body.severity,
        status: body.status,
        description: body.description,
        started_at: body.started_at.unwrap_or(now),
        resolved_at: None,
        created_at: now,
        updated_at: now,
    };
    state.repo.create_incident(&incident).await?;
    Ok((StatusCode::CREATED, Json(incident)))
}

// ---------------------------------------------------------------------------
// PATCH /incidents/:id — update an existing incident
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct PatchIncidentBody {
    pub title: Option<String>,
    pub severity: Option<String>,
    pub status: Option<String>,
    pub description: Option<String>,
    pub resolved_at: Option<Option<DateTime<Utc>>>,
}

pub async fn update_incident(
    State(state): State<AdminState>,
    Path(id): Path<Uuid>,
    Json(body): Json<PatchIncidentBody>,
) -> Result<Json<Incident>, GatewayError> {
    let mut incident = state
        .repo
        .get_incident(&id)
        .await?
        .ok_or(GatewayError::NotFound("incident not found".into()))?;

    if let Some(v) = body.title {
        incident.title = v;
    }
    if let Some(v) = body.severity {
        incident.severity = v;
    }
    if let Some(v) = body.status {
        incident.status = v.clone();
        // Auto-set resolved_at when status becomes "resolved"
        if v == "resolved" && incident.resolved_at.is_none() {
            incident.resolved_at = Some(Utc::now());
        }
    }
    if let Some(v) = body.description {
        incident.description = v;
    }
    if let Some(v) = body.resolved_at {
        incident.resolved_at = v;
    }
    incident.updated_at = Utc::now();

    state.repo.update_incident(&id, &incident).await?;
    Ok(Json(incident))
}

// ---------------------------------------------------------------------------
// DELETE /incidents/:id — delete an incident
// ---------------------------------------------------------------------------

pub async fn delete_incident(
    State(state): State<AdminState>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, GatewayError> {
    if state.repo.delete_incident(&id).await? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(GatewayError::NotFound("incident not found".into()))
    }
}

// ---------------------------------------------------------------------------
// GET /incidents/:id/updates — list updates for an incident
// ---------------------------------------------------------------------------

pub async fn list_incident_updates(
    State(state): State<AdminState>,
    Path(incident_id): Path<Uuid>,
) -> Result<Json<Vec<IncidentUpdate>>, GatewayError> {
    // Verify the incident exists
    state
        .repo
        .get_incident(&incident_id)
        .await?
        .ok_or(GatewayError::NotFound("incident not found".into()))?;
    let updates = state.repo.list_incident_updates(&incident_id).await?;
    Ok(Json(updates))
}

// ---------------------------------------------------------------------------
// POST /incidents/:id/updates — post an update to an incident
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct CreateIncidentUpdateBody {
    #[serde(default = "default_update_status")]
    pub status: String,
    pub message: String,
}

fn default_update_status() -> String {
    "update".into()
}

pub async fn create_incident_update(
    State(state): State<AdminState>,
    Path(incident_id): Path<Uuid>,
    Json(body): Json<CreateIncidentUpdateBody>,
) -> Result<(StatusCode, Json<IncidentUpdate>), GatewayError> {
    // Verify the incident exists
    state
        .repo
        .get_incident(&incident_id)
        .await?
        .ok_or(GatewayError::NotFound("incident not found".into()))?;

    let now = Utc::now();
    let update = IncidentUpdate {
        id: Uuid::new_v4(),
        incident_id,
        status: body.status,
        message: body.message,
        created_at: now,
    };
    state.repo.create_incident_update(&update).await?;
    Ok((StatusCode::CREATED, Json(update)))
}
