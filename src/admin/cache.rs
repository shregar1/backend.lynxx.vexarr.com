use crate::admin::routes::AdminState;
use crate::error::GatewayError;
use crate::models::{CacheEntry, CacheStats};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// GET /services/:id/cache — list cache entries for a service
// ---------------------------------------------------------------------------

pub async fn list_cache_entries(
    State(state): State<AdminState>,
    Path(service_id): Path<Uuid>,
) -> Result<Json<Vec<CacheEntry>>, GatewayError> {
    let entries = state.repo.list_cache_entries(&service_id).await?;
    Ok(Json(entries))
}

// ---------------------------------------------------------------------------
// POST /services/:id/cache/purge — purge all cache for a service
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct PurgeCacheBody {
    #[serde(default)]
    pub keys: Option<Vec<String>>,
}

pub async fn purge_cache(
    State(state): State<AdminState>,
    Path(service_id): Path<Uuid>,
    Json(_body): Json<PurgeCacheBody>,
) -> Result<Json<serde_json::Value>, GatewayError> {
    let purged = state.repo.purge_cache(&service_id).await?;
    Ok(Json(serde_json::json!({
        "purged": true,
        "entries_removed": purged,
        "service_id": service_id,
    })))
}

// ---------------------------------------------------------------------------
// POST /services/:id/cache/warm — warm cache with specific URLs
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct WarmCacheBody {
    pub urls: Vec<String>,
    #[serde(default = "default_ttl")]
    pub ttl_seconds: i32,
}

fn default_ttl() -> i32 {
    3600
}

pub async fn warm_cache(
    State(state): State<AdminState>,
    Path(service_id): Path<Uuid>,
    Json(body): Json<WarmCacheBody>,
) -> Result<(StatusCode, Json<serde_json::Value>), GatewayError> {
    let now = chrono::Utc::now();
    let mut warmed = 0u64;

    for url in &body.urls {
        let entry = CacheEntry {
            id: Uuid::new_v4(),
            service_id,
            cache_key: url.clone(),
            content_hash: format!("{:x}", md5_simple(url)),
            size_bytes: 0,
            ttl_seconds: body.ttl_seconds,
            hits: 0,
            created_at: now,
            expires_at: now + chrono::Duration::seconds(body.ttl_seconds as i64),
        };
        state.repo.upsert_cache_entry(&entry).await?;
        warmed += 1;
    }

    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({
            "warmed": true,
            "entries_created": warmed,
            "service_id": service_id,
        })),
    ))
}

/// Simple hash for cache content deduplication. Not cryptographic.
fn md5_simple(input: &str) -> u64 {
    let mut hash: u64 = 5381;
    for byte in input.bytes() {
        hash = hash.wrapping_mul(33).wrapping_add(byte as u64);
    }
    hash
}

// ---------------------------------------------------------------------------
// GET /services/:id/cache/stats — cache statistics
// ---------------------------------------------------------------------------

pub async fn cache_stats(
    State(state): State<AdminState>,
    Path(service_id): Path<Uuid>,
) -> Result<Json<CacheStats>, GatewayError> {
    let stats = state.repo.cache_stats(&service_id).await?;
    Ok(Json(stats))
}
