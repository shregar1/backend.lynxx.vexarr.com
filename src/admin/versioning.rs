use crate::admin::routes::AdminState;
use crate::error::GatewayError;
use crate::models::ApiVersion;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// GET /services/:id/versions — list API versions for a service
// ---------------------------------------------------------------------------

pub async fn list_versions(
    State(state): State<AdminState>,
    Path(service_id): Path<Uuid>,
) -> Result<Json<Vec<ApiVersion>>, GatewayError> {
    let versions = state.repo.list_api_versions(&service_id).await?;
    Ok(Json(versions))
}

// ---------------------------------------------------------------------------
// POST /services/:id/versions — create a new API version
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct CreateVersionBody {
    pub version: String,
    pub base_path: String,
    #[serde(default)]
    pub upstream_path: String,
    #[serde(default = "default_status")]
    pub status: String,
    #[serde(default)]
    pub deprecated_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub sunset_at: Option<DateTime<Utc>>,
}

fn default_status() -> String {
    "active".into()
}

pub async fn create_version(
    State(state): State<AdminState>,
    Path(service_id): Path<Uuid>,
    Json(body): Json<CreateVersionBody>,
) -> Result<(StatusCode, Json<ApiVersion>), GatewayError> {
    let now = Utc::now();
    let version = ApiVersion {
        id: Uuid::new_v4(),
        service_id,
        version: body.version,
        base_path: body.base_path,
        upstream_path: body.upstream_path,
        status: body.status,
        deprecated_at: body.deprecated_at,
        sunset_at: body.sunset_at,
        created_at: now,
        updated_at: now,
    };
    state.repo.create_api_version(&version).await?;
    Ok((StatusCode::CREATED, Json(version)))
}

// ---------------------------------------------------------------------------
// PATCH /versions/:id — update an existing API version
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct PatchVersionBody {
    pub version: Option<String>,
    pub base_path: Option<String>,
    pub upstream_path: Option<String>,
    pub status: Option<String>,
    pub deprecated_at: Option<Option<DateTime<Utc>>>,
    pub sunset_at: Option<Option<DateTime<Utc>>>,
}

pub async fn patch_version(
    State(state): State<AdminState>,
    Path(id): Path<Uuid>,
    Json(body): Json<PatchVersionBody>,
) -> Result<Json<ApiVersion>, GatewayError> {
    let mut ver = state
        .repo
        .get_api_version(&id)
        .await?
        .ok_or(GatewayError::NotFound("api version not found".into()))?;

    if let Some(v) = body.version {
        ver.version = v;
    }
    if let Some(v) = body.base_path {
        ver.base_path = v;
    }
    if let Some(v) = body.upstream_path {
        ver.upstream_path = v;
    }
    if let Some(v) = body.status {
        ver.status = v;
    }
    if let Some(v) = body.deprecated_at {
        ver.deprecated_at = v;
    }
    if let Some(v) = body.sunset_at {
        ver.sunset_at = v;
    }
    ver.updated_at = Utc::now();

    state.repo.update_api_version(&id, &ver).await?;
    Ok(Json(ver))
}

// ---------------------------------------------------------------------------
// DELETE /versions/:id — delete an API version
// ---------------------------------------------------------------------------

pub async fn delete_version(
    State(state): State<AdminState>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, GatewayError> {
    if state.repo.delete_api_version(&id).await? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(GatewayError::NotFound("api version not found".into()))
    }
}
