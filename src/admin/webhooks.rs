use crate::admin::routes::AdminState;
use crate::error::GatewayError;
use crate::models::{Webhook, WebhookDelivery};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use chrono::Utc;
use serde::Deserialize;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// GET /services/:id/webhooks — list webhooks for a service
// ---------------------------------------------------------------------------

pub async fn list_webhooks(
    State(state): State<AdminState>,
    Path(service_id): Path<Uuid>,
) -> Result<Json<Vec<Webhook>>, GatewayError> {
    let hooks = state.repo.list_webhooks(&service_id).await?;
    Ok(Json(hooks))
}

// ---------------------------------------------------------------------------
// GET /webhooks/:id — get a single webhook
// ---------------------------------------------------------------------------

pub async fn get_webhook(
    State(state): State<AdminState>,
    Path(id): Path<Uuid>,
) -> Result<Json<Webhook>, GatewayError> {
    let hook = state
        .repo
        .get_webhook(&id)
        .await?
        .ok_or(GatewayError::NotFound("webhook not found".into()))?;
    Ok(Json(hook))
}

// ---------------------------------------------------------------------------
// POST /services/:id/webhooks — create a new webhook
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct CreateWebhookBody {
    pub name: String,
    pub url: String,
    #[serde(default = "default_events")]
    pub events: serde_json::Value,
    #[serde(default)]
    pub secret: Option<String>,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(default = "default_retry_count")]
    pub retry_count: i32,
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: i32,
}

fn default_events() -> serde_json::Value {
    serde_json::json!(["*"])
}
fn default_enabled() -> bool {
    true
}
fn default_retry_count() -> i32 {
    3
}
fn default_timeout_ms() -> i32 {
    5000
}

pub async fn create_webhook(
    State(state): State<AdminState>,
    Path(service_id): Path<Uuid>,
    Json(body): Json<CreateWebhookBody>,
) -> Result<(StatusCode, Json<Webhook>), GatewayError> {
    let now = Utc::now();
    let hook = Webhook {
        id: Uuid::new_v4(),
        service_id,
        name: body.name,
        url: body.url,
        events: body.events,
        secret: body.secret,
        enabled: body.enabled,
        retry_count: body.retry_count,
        timeout_ms: body.timeout_ms,
        created_at: now,
        updated_at: now,
    };
    state.repo.create_webhook(&hook).await?;
    Ok((StatusCode::CREATED, Json(hook)))
}

// ---------------------------------------------------------------------------
// PATCH /webhooks/:id — update an existing webhook
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct PatchWebhookBody {
    pub name: Option<String>,
    pub url: Option<String>,
    pub events: Option<serde_json::Value>,
    pub secret: Option<Option<String>>,
    pub enabled: Option<bool>,
    pub retry_count: Option<i32>,
    pub timeout_ms: Option<i32>,
}

pub async fn patch_webhook(
    State(state): State<AdminState>,
    Path(id): Path<Uuid>,
    Json(body): Json<PatchWebhookBody>,
) -> Result<Json<Webhook>, GatewayError> {
    let mut hook = state
        .repo
        .get_webhook(&id)
        .await?
        .ok_or(GatewayError::NotFound("webhook not found".into()))?;

    if let Some(v) = body.name {
        hook.name = v;
    }
    if let Some(v) = body.url {
        hook.url = v;
    }
    if let Some(v) = body.events {
        hook.events = v;
    }
    if let Some(v) = body.secret {
        hook.secret = v;
    }
    if let Some(v) = body.enabled {
        hook.enabled = v;
    }
    if let Some(v) = body.retry_count {
        hook.retry_count = v;
    }
    if let Some(v) = body.timeout_ms {
        hook.timeout_ms = v;
    }
    hook.updated_at = Utc::now();

    state.repo.update_webhook(&id, &hook).await?;
    Ok(Json(hook))
}

// ---------------------------------------------------------------------------
// DELETE /webhooks/:id — delete a webhook
// ---------------------------------------------------------------------------

pub async fn delete_webhook(
    State(state): State<AdminState>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, GatewayError> {
    if state.repo.delete_webhook(&id).await? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(GatewayError::NotFound("webhook not found".into()))
    }
}

// ---------------------------------------------------------------------------
// GET /webhooks/:id/deliveries — list deliveries for a webhook
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct DeliveryQuery {
    #[serde(default = "default_delivery_limit")]
    pub limit: u32,
}

fn default_delivery_limit() -> u32 {
    100
}

pub async fn list_deliveries(
    State(state): State<AdminState>,
    Path(webhook_id): Path<Uuid>,
    Query(q): Query<DeliveryQuery>,
) -> Result<Json<Vec<WebhookDelivery>>, GatewayError> {
    // Verify the webhook exists
    state
        .repo
        .get_webhook(&webhook_id)
        .await?
        .ok_or(GatewayError::NotFound("webhook not found".into()))?;
    let deliveries = state
        .repo
        .list_webhook_deliveries(&webhook_id, q.limit)
        .await?;
    Ok(Json(deliveries))
}

// ---------------------------------------------------------------------------
// POST /webhooks/:id/test — fire a test delivery
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct TestWebhookBody {
    #[serde(default = "default_test_event")]
    pub event_type: String,
    #[serde(default)]
    pub payload: Option<serde_json::Value>,
}

fn default_test_event() -> String {
    "test.ping".into()
}

pub async fn test_webhook(
    State(state): State<AdminState>,
    Path(id): Path<Uuid>,
    Json(body): Json<TestWebhookBody>,
) -> Result<Json<WebhookDelivery>, GatewayError> {
    let hook = state
        .repo
        .get_webhook(&id)
        .await?
        .ok_or(GatewayError::NotFound("webhook not found".into()))?;

    let now = Utc::now();
    let payload = body.payload.unwrap_or_else(|| {
        serde_json::json!({
            "event": body.event_type,
            "test": true,
            "webhook_id": id,
            "timestamp": now.to_rfc3339(),
        })
    });

    // Fire the test request
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_millis(hook.timeout_ms as u64))
        .build()
        .unwrap_or_default();

    let result = client.post(&hook.url).json(&payload).send().await;

    let (response_status, response_body, success) = match result {
        Ok(resp) => {
            let status = resp.status().as_u16() as i32;
            let body = resp.text().await.unwrap_or_default();
            let ok = (200..300).contains(&(status as u16 as usize));
            (Some(status), Some(body), ok)
        }
        Err(e) => (None, Some(e.to_string()), false),
    };

    let delivery = WebhookDelivery {
        id: Uuid::new_v4(),
        webhook_id: id,
        event_type: body.event_type,
        payload,
        response_status,
        response_body,
        success,
        attempts: 1,
        delivered_at: Utc::now(),
    };
    state.repo.create_webhook_delivery(&delivery).await?;

    Ok(Json(delivery))
}
