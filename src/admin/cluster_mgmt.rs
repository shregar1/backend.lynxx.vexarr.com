use crate::admin::routes::AdminState;
use crate::error::GatewayError;
use crate::models::ClusterNode;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use chrono::Utc;
use serde::Deserialize;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// GET /cluster/nodes — list all cluster nodes
// ---------------------------------------------------------------------------

pub async fn list_nodes(
    State(state): State<AdminState>,
) -> Result<Json<Vec<ClusterNode>>, GatewayError> {
    let nodes = state.repo.list_cluster_nodes().await?;
    Ok(Json(nodes))
}

// ---------------------------------------------------------------------------
// GET /cluster/nodes/:id — get a single cluster node
// ---------------------------------------------------------------------------

pub async fn get_node(
    State(state): State<AdminState>,
    Path(id): Path<Uuid>,
) -> Result<Json<ClusterNode>, GatewayError> {
    let node = state
        .repo
        .get_cluster_node(&id)
        .await?
        .ok_or(GatewayError::NotFound("cluster node not found".into()))?;
    Ok(Json(node))
}

// ---------------------------------------------------------------------------
// POST /cluster/nodes/:id/drain — drain a node (stop accepting new connections)
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct DrainNodeBody {
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,
    #[serde(default)]
    pub graceful: bool,
}

fn default_timeout_ms() -> u64 {
    30000
}

pub async fn drain_node(
    State(state): State<AdminState>,
    Path(id): Path<Uuid>,
    Json(body): Json<DrainNodeBody>,
) -> Result<Json<serde_json::Value>, GatewayError> {
    let mut node = state
        .repo
        .get_cluster_node(&id)
        .await?
        .ok_or(GatewayError::NotFound("cluster node not found".into()))?;

    node.status = "draining".into();
    node.last_heartbeat = Utc::now();
    state.repo.upsert_cluster_node(&node).await?;

    Ok(Json(serde_json::json!({
        "draining": true,
        "node_id": id,
        "timeout_ms": body.timeout_ms,
        "graceful": body.graceful,
    })))
}

// ---------------------------------------------------------------------------
// DELETE /cluster/nodes/:id — remove a node from the cluster
// ---------------------------------------------------------------------------

pub async fn remove_node(
    State(state): State<AdminState>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, GatewayError> {
    if state.repo.delete_cluster_node(&id).await? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(GatewayError::NotFound("cluster node not found".into()))
    }
}
