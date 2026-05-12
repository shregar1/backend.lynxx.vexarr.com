use crate::audit::{AuditAction, AuditEntry, AuditFilter, AuditLog, ResourceType};
use crate::auth::middleware::CurrentActor;
use crate::auth::{generate_token, hash_token, AdminToken, Role};
use crate::cluster::{ChangeEvent, ClusterBus};
use crate::db::{AdminTokenPatch, NewAdminToken, Repository};
use crate::error::GatewayError;
use crate::health::{HealthRegistry, ServiceHealth};
use crate::models::{Consumer, PluginInstance, Route, Service, Target};
use crate::plugins::PluginExecutor;
use crate::proxy::http_client::ProxyClientPool;
use crate::proxy::loadbalancer::{LbStrategy, LoadBalancerRegistry};
use crate::proxy::router::SharedRouteTable;
use crate::quota::QuotaTracker;
use crate::reload::{ReloadHandle, ReloadResult};
use axum::body::Body;
use axum::extract::{Extension, Path, Query, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use chrono::{DateTime, Utc};
use metrics_exporter_prometheus::PrometheusHandle;
use serde::{Deserialize, Serialize};
use std::str::FromStr;
use std::sync::Arc;
use uuid::Uuid;

#[derive(Clone)]
pub struct AdminState {
    pub route_table: SharedRouteTable,
    pub repo: Arc<dyn Repository>,
    pub health: Arc<HealthRegistry>,
    pub lb: Arc<LoadBalancerRegistry>,
    pub start_time: chrono::DateTime<Utc>,
    pub metrics_handle: PrometheusHandle,
    pub reload: ReloadHandle,
    pub plugin_exec: Arc<PluginExecutor>,
    pub bus: Arc<dyn ClusterBus>,
    pub node_id: Uuid,
    pub audit: AuditLog,
    /// If true, force authentication regardless of whether any tokens exist
    /// in the DB. When false, the gateway runs in bootstrap mode while the
    /// `admin_tokens` table is empty.
    pub auth_required: bool,
    /// Per-service hyper-util client cache. Mutating service TLS config
    /// here invalidates the corresponding cached entry so the next
    /// request rebuilds with fresh certs.
    pub client_pool: Arc<ProxyClientPool>,
    /// Content-addressed store for full Recording payloads. Used by the
    /// /recordings/:id and /recordings/:id/replay endpoints; the index
    /// (list / delete) lives in the repository.
    pub recording_store: Arc<crate::recording::ContentAddressedStore>,
    /// Reqwest client used by the replay endpoint when firing the
    /// recorded request at a target URL. Separate from the proxy hot-path
    /// client because replays go out-of-band, can be slow, and don't need
    /// the per-service TLS cache.
    pub http_client: reqwest::Client,
    /// Local proxy port. Used to default the replay target URL to "this
    /// gateway" when the operator doesn't pass `target_url` explicitly.
    pub proxy_port: u16,
    /// Monthly quota tracker. Shared with the proxy handler; the admin API
    /// exposes a read-only snapshot at GET /quota.
    pub quota: Arc<QuotaTracker>,
    /// ACME / cert manager. `None` when `[gateway.acme].enabled = false`.
    /// When present, the `/acme/*` admin endpoints delegate challenge
    /// registration here so HTTP-01 *and* TLS-ALPN-01 validators can
    /// run against the live gateway without a restart.
    pub acme: Option<Arc<crate::proxy::acme::AcmeManager>>,
    /// The Lynxx license API key from config/env. Accepted as a valid
    /// master admin credential so customers can use the key they already
    /// have from their account dashboard without creating a separate token.
    pub license_api_key: String,
}

/// Convenience: extract the optional actor from request extensions.
fn actor_of(actor: &CurrentActor) -> Option<&AdminToken> {
    actor.0.as_ref()
}

/// Publish a ChangeEvent on the cluster bus, logging but never failing the
/// admin call if the bus is unhappy. We've already committed to the DB; the
/// remote nodes will eventually reconcile via the next event or restart.
async fn publish_event(state: &AdminState, event: ChangeEvent) {
    if let Err(e) = state.bus.publish(event).await {
        tracing::warn!(error = %e, "failed to publish cluster event");
    }
}

/// Build an ETag from entity id + updated_at.
fn entity_etag(id: &Uuid, updated_at: &DateTime<Utc>) -> String {
    format!("\"{}:{}\"", id.simple(), updated_at.timestamp_millis())
}

/// Return a JSON response with an ETag header.
fn json_with_etag<T: Serialize>(entity: T, etag: &str) -> Response {
    let body = serde_json::to_string(&entity).unwrap();
    Response::builder()
        .status(StatusCode::OK)
        .header(header::ETAG, etag)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body))
        .unwrap()
}

/// Check If-Match header against current ETag. Returns Err(412) on mismatch.
fn check_if_match(
    headers: &HeaderMap,
    current_etag: &str,
) -> Result<(), GatewayError> {
    if let Some(if_match) = headers.get(header::IF_MATCH) {
        let val = if_match.to_str().unwrap_or("");
        if val != "*" && val != current_etag {
            return Err(GatewayError::PreconditionFailed(
                "ETag mismatch — the resource was modified since you last fetched it".into(),
            ));
        }
    }
    Ok(())
}

pub async fn reload_handler(
    State(state): State<AdminState>,
    Extension(actor): Extension<CurrentActor>,
) -> Result<Json<ReloadResult>, GatewayError> {
    let result = state.reload.trigger().await?;
    state
        .audit
        .record(
            AuditAction::Reload,
            ResourceType::Config,
            None,
            None,
            actor_of(&actor),
        )
        .await;
    publish_event(
        &state,
        ChangeEvent::FullSync {
            origin: state.node_id,
        },
    )
    .await;
    Ok(Json(result))
}

#[derive(Deserialize)]
pub struct CreateService {
    pub name: String,
    #[serde(default)]
    pub url: String,
    #[serde(default = "default_retries")]
    pub retries: u32,
    #[serde(default = "default_timeout")]
    pub connect_timeout_ms: u64,
    #[serde(default)]
    pub lb_strategy: Option<String>,
    #[serde(default)]
    pub service_type: Option<String>,
    #[serde(default)]
    pub document_root: Option<String>,
    #[serde(default)]
    pub index_file: Option<String>,
    #[serde(default)]
    pub spa_fallback: Option<bool>,
}

#[derive(Deserialize)]
pub struct CreateTarget {
    pub url: String,
    #[serde(default = "default_weight")]
    pub weight: u32,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub zone: Option<String>,
}

#[derive(Deserialize)]
pub struct UpdateTarget {
    pub url: Option<String>,
    pub weight: Option<u32>,
    pub enabled: Option<bool>,
    pub zone: Option<String>,
}

fn default_weight() -> u32 {
    1
}

fn default_retries() -> u32 {
    3
}
fn default_timeout() -> u64 {
    5000
}

#[derive(Deserialize)]
pub struct CreateRoute {
    pub paths: Vec<String>,
    #[serde(default)]
    pub methods: Vec<String>,
    #[serde(default)]
    pub hosts: Vec<String>,
    pub service_id: Uuid,
    #[serde(default)]
    pub strip_path: bool,
}

#[derive(Deserialize)]
pub struct PatchRoute {
    pub paths: Option<Vec<String>>,
    pub methods: Option<Vec<String>>,
    pub hosts: Option<Vec<String>>,
    pub service_id: Option<Uuid>,
    pub strip_path: Option<bool>,
}

#[derive(Deserialize)]
pub struct PatchService {
    pub name: Option<String>,
    pub url: Option<String>,
    pub retries: Option<u32>,
    pub connect_timeout_ms: Option<u64>,
    pub lb_strategy: Option<String>,
    pub service_type: Option<String>,
    pub document_root: Option<String>,
    pub index_file: Option<String>,
    pub spa_fallback: Option<bool>,
}

#[derive(Deserialize)]
pub struct CreatePlugin {
    pub name: String,
    #[serde(default)]
    pub service_id: Option<Uuid>,
    #[serde(default)]
    pub route_id: Option<Uuid>,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub config: serde_json::Value,
}

fn default_true() -> bool {
    true
}

#[derive(Deserialize)]
pub struct CreateConsumer {
    pub username: String,
    pub custom_id: Option<String>,
}

#[derive(Serialize)]
pub struct StatusResponse {
    pub status: String,
    pub version: String,
    pub build_hash: String,
    pub uptime_seconds: i64,
    pub services_count: usize,
    pub routes_count: usize,
    pub plugins_count: usize,
    pub consumers_count: usize,
}

pub async fn list_services(State(state): State<AdminState>) -> Json<Vec<Service>> {
    Json(state.route_table.list_services())
}

pub async fn create_service(
    State(state): State<AdminState>,
    Extension(actor): Extension<CurrentActor>,
    Json(input): Json<CreateService>,
) -> Result<Json<Service>, GatewayError> {
    let mut service = Service::new(
        input.name,
        input.url,
        input.retries,
        input.connect_timeout_ms,
    );
    if let Some(s) = input.lb_strategy {
        service.lb_strategy = s;
    }
    if let Some(t) = input.service_type {
        service.service_type = t;
    }
    if let Some(root) = input.document_root {
        service.document_root = Some(root);
    }
    if let Some(idx) = input.index_file {
        service.index_file = idx;
    }
    if let Some(spa) = input.spa_fallback {
        service.spa_fallback = spa;
    }
    state.repo.create_service(&service).await?;
    state.route_table.add_service(service.clone());
    state.health.register(service.id);
    state.lb.rebuild_for_service(
        service.id,
        LbStrategy::from_str(&service.lb_strategy),
        Vec::new(),
    );
    tracing::info!(id = %service.id, "service created");
    state
        .audit
        .record(
            AuditAction::Create,
            ResourceType::Service,
            Some(service.id.to_string()),
            serde_json::to_value(&service).ok(),
            actor_of(&actor),
        )
        .await;
    publish_event(
        &state,
        ChangeEvent::ServiceCreated {
            id: service.id,
            origin: state.node_id,
        },
    )
    .await;
    Ok(Json(service))
}

pub async fn get_service(
    State(state): State<AdminState>,
    Path(id): Path<Uuid>,
) -> Result<Response, GatewayError> {
    let svc = state
        .route_table
        .get_service(&id)
        .ok_or_else(|| GatewayError::NotFound(format!("service {id}")))?;
    let etag = entity_etag(&svc.id, &svc.updated_at);
    Ok(json_with_etag(svc, &etag))
}

pub async fn update_service(
    State(state): State<AdminState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    Extension(actor): Extension<CurrentActor>,
    Json(input): Json<CreateService>,
) -> Result<Json<Service>, GatewayError> {
    let now = Utc::now();
    let existing = state
        .route_table
        .get_service(&id)
        .ok_or_else(|| GatewayError::NotFound(format!("service {id}")))?;
    check_if_match(&headers, &entity_etag(&existing.id, &existing.updated_at))?;

    let new_strategy = input
        .lb_strategy
        .clone()
        .unwrap_or_else(|| existing.lb_strategy.clone());
    let strategy_changed = new_strategy != existing.lb_strategy;

    let updated = Service {
        id,
        name: input.name,
        url: input.url,
        retries: input.retries,
        connect_timeout_ms: input.connect_timeout_ms,
        lb_strategy: new_strategy,
        service_type: input
            .service_type
            .unwrap_or_else(|| existing.service_type.clone()),
        document_root: input.document_root.or_else(|| existing.document_root.clone()),
        index_file: input
            .index_file
            .unwrap_or_else(|| existing.index_file.clone()),
        spa_fallback: input.spa_fallback.unwrap_or(existing.spa_fallback),
        tls_verify: existing.tls_verify,
        tls_client_cert_path: existing.tls_client_cert_path.clone(),
        tls_client_key_path: existing.tls_client_key_path.clone(),
        tls_ca_path: existing.tls_ca_path.clone(),
        lb_hash_header: existing.lb_hash_header.clone(),
        lb_hash_query_param: existing.lb_hash_query_param.clone(),
        flush_interval_ms: existing.flush_interval_ms,
        stream_timeout_ms: existing.stream_timeout_ms,
        stream_close_delay_ms: existing.stream_close_delay_ms,
        created_at: existing.created_at,
        updated_at: now,
    };

    state.repo.update_service(&id, &updated).await?;
    state.route_table.update_service(&id, updated.clone());
    state.client_pool.invalidate(&id);

    if strategy_changed {
        let targets = state.repo.list_targets_for_service(&id).await?;
        state
            .lb
            .rebuild_for_service(id, LbStrategy::from_str(&updated.lb_strategy), targets);
    }

    state
        .audit
        .record(
            AuditAction::Update,
            ResourceType::Service,
            Some(id.to_string()),
            Some(serde_json::json!({
                "before": existing,
                "after": updated,
            })),
            actor_of(&actor),
        )
        .await;
    publish_event(
        &state,
        ChangeEvent::ServiceUpdated {
            id,
            origin: state.node_id,
        },
    )
    .await;
    Ok(Json(updated))
}

pub async fn delete_service(
    State(state): State<AdminState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    Extension(actor): Extension<CurrentActor>,
) -> Result<Json<serde_json::Value>, GatewayError> {
    if let Some(existing) = state.route_table.get_service(&id) {
        check_if_match(&headers, &entity_etag(&existing.id, &existing.updated_at))?;
    }
    let deleted = state.repo.delete_service(&id).await?;
    if deleted {
        state.route_table.remove_service(&id);
        state.health.unregister(&id);
        state.lb.remove_service(&id);
        state.client_pool.invalidate(&id);
        state
            .audit
            .record(
                AuditAction::Delete,
                ResourceType::Service,
                Some(id.to_string()),
                None,
                actor_of(&actor),
            )
            .await;
        publish_event(
            &state,
            ChangeEvent::ServiceDeleted {
                id,
                origin: state.node_id,
            },
        )
        .await;
        Ok(Json(serde_json::json!({"deleted": true})))
    } else {
        Err(GatewayError::NotFound(format!("service {id}")))
    }
}

pub async fn list_health(State(state): State<AdminState>) -> Json<Vec<ServiceHealth>> {
    Json(state.health.list())
}

pub async fn get_health(
    State(state): State<AdminState>,
    Path(id): Path<Uuid>,
) -> Result<Response, GatewayError> {
    let health = state
        .health
        .get(&id)
        .ok_or_else(|| GatewayError::NotFound(format!("health for service {id}")))?;
    let ts = health
        .last_check_at
        .unwrap_or_else(|| DateTime::<Utc>::UNIX_EPOCH);
    let etag = entity_etag(&health.service_id, &ts);
    Ok(json_with_etag(health, &etag))
}

// ── Upstream status API ──

/// Per-target snapshot returned by `GET /admin/upstreams`.
#[derive(Serialize)]
pub struct UpstreamTargetStatus {
    pub target_id: Uuid,
    pub service_id: Uuid,
    pub service_name: String,
    pub url: String,
    pub weight: u32,
    pub enabled: bool,
    pub healthy: bool,
    pub in_flight: u64,
    pub zone: Option<String>,
}

/// Aggregate response for `GET /admin/upstreams`.
#[derive(Serialize)]
pub struct UpstreamStatusResponse {
    pub total_targets: usize,
    pub healthy_targets: usize,
    pub unhealthy_targets: usize,
    pub targets: Vec<UpstreamTargetStatus>,
}

/// `GET /admin/upstreams` — live snapshot of all upstream targets across
/// every service, including health, in-flight count, and weight.
pub async fn list_upstreams(State(state): State<AdminState>) -> Json<UpstreamStatusResponse> {
    let services = state.route_table.list_services();
    let mut targets = Vec::new();

    for svc in &services {
        let snapshot = state.lb.snapshot(&svc.id);
        for ts in snapshot {
            targets.push(UpstreamTargetStatus {
                target_id: ts.target.id,
                service_id: svc.id,
                service_name: svc.name.clone(),
                url: ts.target.url.clone(),
                weight: ts.target.weight,
                enabled: ts.target.enabled,
                healthy: ts.healthy.load(std::sync::atomic::Ordering::Relaxed),
                in_flight: ts.in_flight.load(std::sync::atomic::Ordering::Relaxed),
                zone: ts.target.zone.clone(),
            });
        }
    }

    let healthy = targets.iter().filter(|t| t.healthy && t.enabled).count();
    let unhealthy = targets.iter().filter(|t| !t.healthy && t.enabled).count();
    let total = targets.len();

    Json(UpstreamStatusResponse {
        total_targets: total,
        healthy_targets: healthy,
        unhealthy_targets: unhealthy,
        targets,
    })
}

pub async fn list_routes(State(state): State<AdminState>) -> Json<Vec<Route>> {
    Json(state.route_table.list_routes())
}

pub async fn create_route(
    State(state): State<AdminState>,
    Extension(actor): Extension<CurrentActor>,
    Json(input): Json<CreateRoute>,
) -> Result<Json<Route>, GatewayError> {
    if state.route_table.get_service(&input.service_id).is_none() {
        return Err(GatewayError::BadRequest(format!(
            "service {} not found",
            input.service_id
        )));
    }

    let route = Route::new(
        input.paths,
        input.methods,
        input.hosts,
        input.service_id,
        input.strip_path,
    );
    state.repo.create_route(&route).await?;
    state.route_table.add_route(route.clone());
    tracing::info!(id = %route.id, "route created");
    state
        .audit
        .record(
            AuditAction::Create,
            ResourceType::Route,
            Some(route.id.to_string()),
            serde_json::to_value(&route).ok(),
            actor_of(&actor),
        )
        .await;
    publish_event(
        &state,
        ChangeEvent::RouteCreated {
            id: route.id,
            origin: state.node_id,
        },
    )
    .await;
    Ok(Json(route))
}

pub async fn get_route(
    State(state): State<AdminState>,
    Path(id): Path<Uuid>,
) -> Result<Response, GatewayError> {
    let route = state
        .route_table
        .get_route(&id)
        .ok_or_else(|| GatewayError::NotFound(format!("route {id}")))?;
    let etag = entity_etag(&route.id, &route.updated_at);
    Ok(json_with_etag(route, &etag))
}

pub async fn update_route(
    State(state): State<AdminState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    Extension(actor): Extension<CurrentActor>,
    Json(input): Json<CreateRoute>,
) -> Result<Json<Route>, GatewayError> {
    let now = Utc::now();
    let existing = state
        .route_table
        .get_route(&id)
        .ok_or_else(|| GatewayError::NotFound(format!("route {id}")))?;
    check_if_match(&headers, &entity_etag(&existing.id, &existing.updated_at))?;

    let updated = Route {
        id,
        paths: input.paths,
        methods: input.methods,
        hosts: input.hosts,
        service_id: input.service_id,
        strip_path: input.strip_path,
        buffer_request: false,
        path_rewrite_pattern: None,
        path_rewrite_replacement: None,
        headers: std::collections::HashMap::new(),
        query_params: std::collections::HashMap::new(),
        path_regexes: Vec::new(),
        read_timeout_ms: None,
        write_timeout_ms: None,
        idle_timeout_ms: None,
        retry_on_status: Vec::new(),
        matchers: Vec::new(),
        matcher_refs: Vec::new(),
        handle_blocks: Vec::new(),
        try_files: None,
        uri_directives: Vec::new(),
        intercept_rules: Vec::new(),
        push: None,
        static_response: None,
        created_at: existing.created_at,
        updated_at: now,
    };

    state.repo.update_route(&id, &updated).await?;
    state.route_table.update_route(&id, updated.clone());
    state
        .audit
        .record(
            AuditAction::Update,
            ResourceType::Route,
            Some(id.to_string()),
            Some(serde_json::json!({ "before": existing, "after": updated })),
            actor_of(&actor),
        )
        .await;
    publish_event(
        &state,
        ChangeEvent::RouteUpdated {
            id,
            origin: state.node_id,
        },
    )
    .await;
    Ok(Json(updated))
}

pub async fn patch_route(
    State(state): State<AdminState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    Extension(actor): Extension<CurrentActor>,
    Json(input): Json<PatchRoute>,
) -> Result<Json<Route>, GatewayError> {
    let now = Utc::now();
    let existing = state
        .route_table
        .get_route(&id)
        .ok_or_else(|| GatewayError::NotFound(format!("route {id}")))?;
    check_if_match(&headers, &entity_etag(&existing.id, &existing.updated_at))?;

    if let Some(ref sid) = input.service_id {
        if state.route_table.get_service(sid).is_none() {
            return Err(GatewayError::BadRequest(format!("service {} not found", sid)));
        }
    }

    let updated = Route {
        id,
        paths: input.paths.unwrap_or_else(|| existing.paths.clone()),
        methods: input.methods.unwrap_or_else(|| existing.methods.clone()),
        hosts: input.hosts.unwrap_or_else(|| existing.hosts.clone()),
        service_id: input.service_id.unwrap_or(existing.service_id),
        strip_path: input.strip_path.unwrap_or(existing.strip_path),
        buffer_request: existing.buffer_request,
        path_rewrite_pattern: existing.path_rewrite_pattern.clone(),
        path_rewrite_replacement: existing.path_rewrite_replacement.clone(),
        headers: existing.headers.clone(),
        query_params: existing.query_params.clone(),
        path_regexes: existing.path_regexes.clone(),
        read_timeout_ms: existing.read_timeout_ms,
        write_timeout_ms: existing.write_timeout_ms,
        idle_timeout_ms: existing.idle_timeout_ms,
        retry_on_status: existing.retry_on_status.clone(),
        matchers: existing.matchers.clone(),
        matcher_refs: existing.matcher_refs.clone(),
        handle_blocks: existing.handle_blocks.clone(),
        try_files: existing.try_files.clone(),
        uri_directives: existing.uri_directives.clone(),
        intercept_rules: existing.intercept_rules.clone(),
        push: existing.push.clone(),
        static_response: existing.static_response.clone(),
        created_at: existing.created_at,
        updated_at: now,
    };

    state.repo.update_route(&id, &updated).await?;
    state.route_table.update_route(&id, updated.clone());
    state
        .audit
        .record(
            AuditAction::Update,
            ResourceType::Route,
            Some(id.to_string()),
            Some(serde_json::json!({ "before": existing, "after": updated })),
            actor_of(&actor),
        )
        .await;
    publish_event(
        &state,
        ChangeEvent::RouteUpdated {
            id,
            origin: state.node_id,
        },
    )
    .await;
    Ok(Json(updated))
}

pub async fn patch_service(
    State(state): State<AdminState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    Extension(actor): Extension<CurrentActor>,
    Json(input): Json<PatchService>,
) -> Result<Json<Service>, GatewayError> {
    let now = Utc::now();
    let existing = state
        .route_table
        .get_service(&id)
        .ok_or_else(|| GatewayError::NotFound(format!("service {id}")))?;
    check_if_match(&headers, &entity_etag(&existing.id, &existing.updated_at))?;

    let new_strategy = input
        .lb_strategy
        .unwrap_or_else(|| existing.lb_strategy.clone());
    let strategy_changed = new_strategy != existing.lb_strategy;

    let updated = Service {
        id,
        name: input.name.unwrap_or_else(|| existing.name.clone()),
        url: input.url.unwrap_or_else(|| existing.url.clone()),
        retries: input.retries.unwrap_or(existing.retries),
        connect_timeout_ms: input.connect_timeout_ms.unwrap_or(existing.connect_timeout_ms),
        lb_strategy: new_strategy,
        service_type: input
            .service_type
            .unwrap_or_else(|| existing.service_type.clone()),
        document_root: input.document_root.or_else(|| existing.document_root.clone()),
        index_file: input
            .index_file
            .unwrap_or_else(|| existing.index_file.clone()),
        spa_fallback: input.spa_fallback.unwrap_or(existing.spa_fallback),
        tls_verify: existing.tls_verify,
        tls_client_cert_path: existing.tls_client_cert_path.clone(),
        tls_client_key_path: existing.tls_client_key_path.clone(),
        tls_ca_path: existing.tls_ca_path.clone(),
        lb_hash_header: existing.lb_hash_header.clone(),
        lb_hash_query_param: existing.lb_hash_query_param.clone(),
        flush_interval_ms: existing.flush_interval_ms,
        stream_timeout_ms: existing.stream_timeout_ms,
        stream_close_delay_ms: existing.stream_close_delay_ms,
        created_at: existing.created_at,
        updated_at: now,
    };

    state.repo.update_service(&id, &updated).await?;
    state.route_table.update_service(&id, updated.clone());
    state.client_pool.invalidate(&id);

    if strategy_changed {
        let targets = state.repo.list_targets_for_service(&id).await?;
        state
            .lb
            .rebuild_for_service(id, LbStrategy::from_str(&updated.lb_strategy), targets);
    }

    state
        .audit
        .record(
            AuditAction::Update,
            ResourceType::Service,
            Some(id.to_string()),
            Some(serde_json::json!({ "before": existing, "after": updated })),
            actor_of(&actor),
        )
        .await;
    publish_event(
        &state,
        ChangeEvent::ServiceUpdated {
            id,
            origin: state.node_id,
        },
    )
    .await;
    Ok(Json(updated))
}

pub async fn delete_route(
    State(state): State<AdminState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    Extension(actor): Extension<CurrentActor>,
) -> Result<Json<serde_json::Value>, GatewayError> {
    if let Some(existing) = state.route_table.get_route(&id) {
        check_if_match(&headers, &entity_etag(&existing.id, &existing.updated_at))?;
    }
    let deleted = state.repo.delete_route(&id).await?;
    if deleted {
        state.route_table.remove_route(&id);
        state
            .audit
            .record(
                AuditAction::Delete,
                ResourceType::Route,
                Some(id.to_string()),
                None,
                actor_of(&actor),
            )
            .await;
        publish_event(
            &state,
            ChangeEvent::RouteDeleted {
                id,
                origin: state.node_id,
            },
        )
        .await;
        Ok(Json(serde_json::json!({"deleted": true})))
    } else {
        Err(GatewayError::NotFound(format!("route {id}")))
    }
}

pub async fn list_plugins(
    State(state): State<AdminState>,
) -> Result<Json<Vec<PluginInstance>>, GatewayError> {
    Ok(Json(state.repo.list_plugins().await?))
}

pub async fn create_plugin(
    State(state): State<AdminState>,
    Extension(actor): Extension<CurrentActor>,
    Json(input): Json<CreatePlugin>,
) -> Result<Json<PluginInstance>, GatewayError> {
    validate_plugin_scope(&state, input.service_id, input.route_id).await?;
    // Eagerly validate the plugin can be built before persisting.
    let _ = crate::plugins::factory::build(&input.name, &input.config, Some(state.repo.clone()))?;
    let now = Utc::now();
    let plugin = PluginInstance {
        id: Uuid::new_v4(),
        name: input.name,
        service_id: input.service_id,
        route_id: input.route_id,
        enabled: input.enabled,
        config: input.config,
        created_at: now,
        updated_at: now,
    };
    state.repo.create_plugin(&plugin).await?;
    state.plugin_exec.rebuild(&state.repo).await?;
    state
        .audit
        .record(
            AuditAction::Create,
            ResourceType::Plugin,
            Some(plugin.id.to_string()),
            serde_json::to_value(&plugin).ok(),
            actor_of(&actor),
        )
        .await;
    publish_event(
        &state,
        ChangeEvent::PluginChanged {
            origin: state.node_id,
        },
    )
    .await;
    Ok(Json(plugin))
}

pub async fn get_plugin(
    State(state): State<AdminState>,
    Path(id): Path<Uuid>,
) -> Result<Response, GatewayError> {
    let plugin = state
        .repo
        .get_plugin(&id)
        .await?
        .ok_or_else(|| GatewayError::NotFound(format!("plugin {id}")))?;
    let etag = entity_etag(&plugin.id, &plugin.updated_at);
    Ok(json_with_etag(plugin, &etag))
}

pub async fn update_plugin(
    State(state): State<AdminState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    Extension(actor): Extension<CurrentActor>,
    Json(input): Json<CreatePlugin>,
) -> Result<Json<PluginInstance>, GatewayError> {
    let existing = state
        .repo
        .get_plugin(&id)
        .await?
        .ok_or_else(|| GatewayError::NotFound(format!("plugin {id}")))?;
    check_if_match(&headers, &entity_etag(&existing.id, &existing.updated_at))?;

    validate_plugin_scope(&state, input.service_id, input.route_id).await?;
    let _ = crate::plugins::factory::build(&input.name, &input.config, Some(state.repo.clone()))?;

    let updated = PluginInstance {
        id: existing.id,
        name: input.name,
        service_id: input.service_id,
        route_id: input.route_id,
        enabled: input.enabled,
        config: input.config,
        created_at: existing.created_at,
        updated_at: Utc::now(),
    };
    state.repo.update_plugin(&id, &updated).await?;
    state.plugin_exec.rebuild(&state.repo).await?;
    state
        .audit
        .record(
            AuditAction::Update,
            ResourceType::Plugin,
            Some(id.to_string()),
            Some(serde_json::json!({ "before": existing, "after": updated })),
            actor_of(&actor),
        )
        .await;
    publish_event(
        &state,
        ChangeEvent::PluginChanged {
            origin: state.node_id,
        },
    )
    .await;
    Ok(Json(updated))
}

pub async fn delete_plugin(
    State(state): State<AdminState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    Extension(actor): Extension<CurrentActor>,
) -> Result<Json<serde_json::Value>, GatewayError> {
    let existing = state
        .repo
        .get_plugin(&id)
        .await?
        .ok_or_else(|| GatewayError::NotFound(format!("plugin {id}")))?;
    check_if_match(&headers, &entity_etag(&existing.id, &existing.updated_at))?;
    let deleted = state.repo.delete_plugin(&id).await?;
    if deleted {
        state.plugin_exec.rebuild(&state.repo).await?;
        state
            .audit
            .record(
                AuditAction::Delete,
                ResourceType::Plugin,
                Some(id.to_string()),
                None,
                actor_of(&actor),
            )
            .await;
        publish_event(
            &state,
            ChangeEvent::PluginChanged {
                origin: state.node_id,
            },
        )
        .await;
        Ok(Json(serde_json::json!({"deleted": true})))
    } else {
        Err(GatewayError::NotFound(format!("plugin {id}")))
    }
}

async fn validate_plugin_scope(
    state: &AdminState,
    service_id: Option<Uuid>,
    route_id: Option<Uuid>,
) -> Result<(), GatewayError> {
    if let Some(sid) = service_id {
        if state.route_table.get_service(&sid).is_none() {
            return Err(GatewayError::BadRequest(format!("service {sid} not found")));
        }
    }
    if let Some(rid) = route_id {
        if state.route_table.get_route(&rid).is_none() {
            return Err(GatewayError::BadRequest(format!("route {rid} not found")));
        }
    }
    Ok(())
}

pub async fn list_consumers(
    State(state): State<AdminState>,
) -> Result<Json<Vec<Consumer>>, GatewayError> {
    Ok(Json(state.repo.list_consumers().await?))
}

pub async fn create_consumer(
    State(state): State<AdminState>,
    Extension(actor): Extension<CurrentActor>,
    Json(input): Json<CreateConsumer>,
) -> Result<Json<Consumer>, GatewayError> {
    let consumer = Consumer::new(input.username, input.custom_id);
    state.repo.create_consumer(&consumer).await?;
    state
        .audit
        .record(
            AuditAction::Create,
            ResourceType::Consumer,
            Some(consumer.id.to_string()),
            serde_json::to_value(&consumer).ok(),
            actor_of(&actor),
        )
        .await;
    publish_event(
        &state,
        ChangeEvent::ConsumerChanged {
            origin: state.node_id,
        },
    )
    .await;
    Ok(Json(consumer))
}

pub async fn metrics(State(state): State<AdminState>) -> Response {
    let body = state.metrics_handle.render();
    (
        StatusCode::OK,
        [(
            header::CONTENT_TYPE,
            HeaderValue::from_static("text/plain; version=0.0.4"),
        )],
        body,
    )
        .into_response()
}

// ---------- targets ----------

async fn rebuild_lb(state: &AdminState, service_id: Uuid) -> Result<(), GatewayError> {
    let svc = state
        .route_table
        .get_service(&service_id)
        .ok_or_else(|| GatewayError::NotFound(format!("service {service_id}")))?;
    let targets = state.repo.list_targets_for_service(&service_id).await?;
    state
        .lb
        .rebuild_for_service(service_id, LbStrategy::from_str(&svc.lb_strategy), targets);
    Ok(())
}

pub async fn list_all_targets(
    State(state): State<AdminState>,
) -> Result<Json<Vec<Target>>, GatewayError> {
    Ok(Json(state.repo.list_targets().await?))
}

pub async fn list_service_targets(
    State(state): State<AdminState>,
    Path(service_id): Path<Uuid>,
) -> Result<Json<Vec<Target>>, GatewayError> {
    if state.route_table.get_service(&service_id).is_none() {
        return Err(GatewayError::NotFound(format!("service {service_id}")));
    }
    Ok(Json(
        state.repo.list_targets_for_service(&service_id).await?,
    ))
}

pub async fn create_service_target(
    State(state): State<AdminState>,
    Path(service_id): Path<Uuid>,
    Extension(actor): Extension<CurrentActor>,
    Json(input): Json<CreateTarget>,
) -> Result<Json<Target>, GatewayError> {
    if state.route_table.get_service(&service_id).is_none() {
        return Err(GatewayError::NotFound(format!("service {service_id}")));
    }
    let mut target = Target::new(service_id, input.url, input.weight);
    target.enabled = input.enabled;
    target.zone = input.zone;
    state.repo.create_target(&target).await?;
    rebuild_lb(&state, service_id).await?;
    tracing::info!(target_id = %target.id, service_id = %service_id, "target created");
    state
        .audit
        .record(
            AuditAction::Create,
            ResourceType::Target,
            Some(target.id.to_string()),
            serde_json::to_value(&target).ok(),
            actor_of(&actor),
        )
        .await;
    publish_event(
        &state,
        ChangeEvent::TargetChanged {
            service_id,
            origin: state.node_id,
        },
    )
    .await;
    Ok(Json(target))
}

pub async fn update_target_handler(
    State(state): State<AdminState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    Extension(actor): Extension<CurrentActor>,
    Json(input): Json<UpdateTarget>,
) -> Result<Json<Target>, GatewayError> {
    let existing = state
        .repo
        .get_target(&id)
        .await?
        .ok_or_else(|| GatewayError::NotFound(format!("target {id}")))?;
    check_if_match(&headers, &entity_etag(&existing.id, &existing.updated_at))?;

    let updated = Target {
        id: existing.id,
        service_id: existing.service_id,
        url: input.url.clone().unwrap_or_else(|| existing.url.clone()),
        weight: input.weight.unwrap_or(existing.weight).max(1),
        enabled: input.enabled.unwrap_or(existing.enabled),
        zone: input.zone.or(existing.zone.clone()),
        created_at: existing.created_at,
        updated_at: Utc::now(),
    };

    state.repo.update_target(&id, &updated).await?;
    rebuild_lb(&state, updated.service_id).await?;
    state
        .audit
        .record(
            AuditAction::Update,
            ResourceType::Target,
            Some(id.to_string()),
            Some(serde_json::json!({ "before": existing, "after": updated })),
            actor_of(&actor),
        )
        .await;
    publish_event(
        &state,
        ChangeEvent::TargetChanged {
            service_id: updated.service_id,
            origin: state.node_id,
        },
    )
    .await;
    Ok(Json(updated))
}

pub async fn delete_target_handler(
    State(state): State<AdminState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    Extension(actor): Extension<CurrentActor>,
) -> Result<Json<serde_json::Value>, GatewayError> {
    let existing = state
        .repo
        .get_target(&id)
        .await?
        .ok_or_else(|| GatewayError::NotFound(format!("target {id}")))?;
    check_if_match(&headers, &entity_etag(&existing.id, &existing.updated_at))?;
    let deleted = state.repo.delete_target(&id).await?;
    if deleted {
        rebuild_lb(&state, existing.service_id).await?;
        state
            .audit
            .record(
                AuditAction::Delete,
                ResourceType::Target,
                Some(id.to_string()),
                None,
                actor_of(&actor),
            )
            .await;
        publish_event(
            &state,
            ChangeEvent::TargetChanged {
                service_id: existing.service_id,
                origin: state.node_id,
            },
        )
        .await;
        Ok(Json(serde_json::json!({"deleted": true})))
    } else {
        Err(GatewayError::NotFound(format!("target {id}")))
    }
}

// ---------- admin tokens ----------

#[derive(Serialize)]
pub struct AdminTokenView {
    pub id: Uuid,
    pub name: String,
    pub role: Role,
    pub created_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
    pub last_used_at: Option<DateTime<Utc>>,
}

impl From<AdminToken> for AdminTokenView {
    fn from(t: AdminToken) -> Self {
        Self {
            id: t.id,
            name: t.name,
            role: t.role,
            created_at: t.created_at,
            expires_at: t.expires_at,
            last_used_at: t.last_used_at,
        }
    }
}

#[derive(Deserialize)]
pub struct CreateTokenInput {
    pub name: String,
    pub role: Role,
    pub expires_at: Option<DateTime<Utc>>,
}

#[derive(Serialize)]
pub struct CreatedTokenResponse {
    #[serde(flatten)]
    pub view: AdminTokenView,
    /// Raw token. Returned ONCE on creation; never retrievable again.
    pub token: String,
}

#[derive(Deserialize)]
pub struct UpdateTokenInput {
    pub name: Option<String>,
    pub role: Option<Role>,
    /// `null` clears the expiry; field absence leaves it unchanged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<Option<DateTime<Utc>>>,
}

pub async fn list_tokens(
    State(state): State<AdminState>,
) -> Result<Json<Vec<AdminTokenView>>, GatewayError> {
    let tokens = state.repo.list_admin_tokens().await?;
    Ok(Json(tokens.into_iter().map(Into::into).collect()))
}

pub async fn create_token(
    State(state): State<AdminState>,
    Extension(actor): Extension<CurrentActor>,
    Json(input): Json<CreateTokenInput>,
) -> Result<Json<CreatedTokenResponse>, GatewayError> {
    if input.name.trim().is_empty() {
        return Err(GatewayError::BadRequest("token name required".into()));
    }
    let raw = generate_token();
    let new = NewAdminToken {
        id: Uuid::new_v4(),
        name: input.name,
        token_hash: hash_token(&raw),
        role: input.role,
        created_at: Utc::now(),
        expires_at: input.expires_at,
    };
    state.repo.create_admin_token(&new).await?;
    let view = AdminTokenView {
        id: new.id,
        name: new.name.clone(),
        role: new.role,
        created_at: new.created_at,
        expires_at: new.expires_at,
        last_used_at: None,
    };
    state
        .audit
        .record(
            AuditAction::Create,
            ResourceType::Token,
            Some(new.id.to_string()),
            Some(serde_json::json!({
                "name": new.name,
                "role": new.role,
                "expires_at": new.expires_at,
            })),
            actor_of(&actor),
        )
        .await;
    Ok(Json(CreatedTokenResponse { view, token: raw }))
}

pub async fn update_token(
    State(state): State<AdminState>,
    Path(id): Path<Uuid>,
    Extension(actor): Extension<CurrentActor>,
    Json(input): Json<UpdateTokenInput>,
) -> Result<Json<AdminTokenView>, GatewayError> {
    let patch = AdminTokenPatch {
        name: input.name,
        role: input.role,
        expires_at: input.expires_at,
    };
    let updated = state
        .repo
        .update_admin_token(&id, &patch)
        .await?
        .ok_or_else(|| GatewayError::NotFound(format!("token {id}")))?;
    state
        .audit
        .record(
            AuditAction::Update,
            ResourceType::Token,
            Some(id.to_string()),
            Some(serde_json::json!({
                "name": updated.name,
                "role": updated.role,
                "expires_at": updated.expires_at,
            })),
            actor_of(&actor),
        )
        .await;
    Ok(Json(updated.into()))
}

pub async fn delete_token(
    State(state): State<AdminState>,
    Path(id): Path<Uuid>,
    Extension(actor): Extension<CurrentActor>,
) -> Result<Json<serde_json::Value>, GatewayError> {
    let deleted = state.repo.delete_admin_token(&id).await?;
    if !deleted {
        return Err(GatewayError::NotFound(format!("token {id}")));
    }
    state
        .audit
        .record(
            AuditAction::Delete,
            ResourceType::Token,
            Some(id.to_string()),
            None,
            actor_of(&actor),
        )
        .await;
    Ok(Json(serde_json::json!({"deleted": true})))
}

// ---------- audit log ----------

#[derive(Deserialize)]
pub struct AuditQuery {
    pub actor: Option<String>,
    pub action: Option<String>,
    pub resource_type: Option<String>,
    pub since: Option<DateTime<Utc>>,
    pub limit: Option<u32>,
    #[serde(default)]
    pub include_diff: bool,
}

pub async fn list_audit(
    State(state): State<AdminState>,
    Query(q): Query<AuditQuery>,
) -> Result<Json<Vec<AuditEntry>>, GatewayError> {
    let filter = AuditFilter {
        actor: q.actor,
        action: q.action,
        resource_type: q.resource_type,
        since: q.since,
        limit: q.limit,
        include_diff: q.include_diff,
    };
    Ok(Json(state.audit.query(&filter).await?))
}

pub async fn verify_audit(
    State(state): State<AdminState>,
) -> Result<Json<serde_json::Value>, GatewayError> {
    let result = state.audit.verify_chain().await?;
    Ok(Json(serde_json::json!({
        "ok": result.ok,
        "checked": result.checked,
        "broken_at": result.broken_at,
    })))
}

// Quiet unused-import warnings for items wired by mod.rs only.
#[allow(dead_code)]
const _USE_ROLE: fn() -> Role = || Role::Admin;
#[allow(dead_code)]
const _USE_FROMSTR: fn(&str) -> Result<Role, String> = Role::from_str;

// ---------- recordings (Time-Travel Debug A3) ----------

#[derive(Deserialize)]
pub struct ListRecordingsQuery {
    pub since: Option<DateTime<Utc>>,
    pub method: Option<String>,
    pub path_prefix: Option<String>,
    pub status: Option<u16>,
    pub service_id: Option<Uuid>,
    pub limit: Option<usize>,
}

#[derive(Deserialize, Default)]
pub struct ReplayBody {
    /// Optional override. When `None`, replays go through the local proxy.
    pub target_url: Option<String>,
}

pub async fn list_recordings(
    State(state): State<AdminState>,
    Query(q): Query<ListRecordingsQuery>,
) -> Result<Json<Vec<crate::recording::RecordingIndexRow>>, GatewayError> {
    let filter = crate::recording::RecordingFilter {
        since: q.since,
        method: q.method,
        path_prefix: q.path_prefix,
        status: q.status,
        service_id: q.service_id,
        limit: q.limit.unwrap_or(0),
    };
    let rows = state.repo.list_recordings(&filter).await?;
    Ok(Json(rows))
}

pub async fn get_recording(
    State(state): State<AdminState>,
    Path(id): Path<String>,
) -> Result<Json<crate::recording::Recording>, GatewayError> {
    match state.recording_store.get(&id).await? {
        Some(rec) => Ok(Json(rec)),
        None => Err(GatewayError::NotFound(format!("recording {id}"))),
    }
}

pub async fn delete_recording_handler(
    State(state): State<AdminState>,
    Path(id): Path<String>,
) -> Result<StatusCode, GatewayError> {
    let store_deleted = state.recording_store.delete(&id).await?;
    let index_deleted = state.repo.delete_recording(&id).await?;
    if store_deleted || index_deleted {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(GatewayError::NotFound(format!("recording {id}")))
    }
}

pub async fn replay_recording(
    State(state): State<AdminState>,
    Path(id): Path<String>,
    Json(body): Json<ReplayBody>,
) -> Result<Json<crate::recording::ReplayResult>, GatewayError> {
    let rec = state
        .recording_store
        .get(&id)
        .await?
        .ok_or_else(|| GatewayError::NotFound(format!("recording {id}")))?;
    let target_url = body
        .target_url
        .unwrap_or_else(|| format!("http://127.0.0.1:{}", state.proxy_port));
    let result = crate::recording::replay(&rec, &target_url, &state.http_client).await?;
    Ok(Json(result))
}

pub async fn gateway_status(
    State(state): State<AdminState>,
) -> Result<Json<StatusResponse>, GatewayError> {
    let uptime = Utc::now()
        .signed_duration_since(state.start_time)
        .num_seconds();

    let plugins_count = state.repo.list_plugins().await?.len();
    let consumers_count = state.repo.list_consumers().await?.len();

    Ok(Json(StatusResponse {
        status: "running".into(),
        version: env!("CARGO_PKG_VERSION").into(),
        build_hash: env!("LYNXX_BUILD_HASH").into(),
        uptime_seconds: uptime,
        services_count: state.route_table.list_services().len(),
        routes_count: state.route_table.list_routes().len(),
        plugins_count,
        consumers_count,
    }))
}

pub async fn healthz(
    State(state): State<AdminState>,
) -> Result<Json<StatusResponse>, GatewayError> {
    gateway_status(State(state)).await
}

// ---------- circuit breakers ----------

pub async fn list_circuit_breakers(
    State(state): State<AdminState>,
) -> Json<Vec<ServiceHealth>> {
    Json(state.health.list())
}

pub async fn reset_circuit_breaker(
    State(state): State<AdminState>,
    Path(service_id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, GatewayError> {
    state
        .health
        .reset_circuit(&service_id)
        .ok_or_else(|| GatewayError::NotFound(format!("service {service_id}")))?;
    tracing::info!(service_id = %service_id, "circuit breaker manually reset");
    Ok(Json(serde_json::json!({"reset": true, "service_id": service_id})))
}

// ---------- ACME force renew ----------

#[derive(Deserialize)]
pub struct AcmeRenewQuery {
    pub force: Option<bool>,
}

pub async fn acme_renew(
    State(state): State<AdminState>,
    Query(q): Query<AcmeRenewQuery>,
) -> Result<Json<serde_json::Value>, GatewayError> {
    let mgr = require_acme(&state)?;
    if q.force.unwrap_or(false) {
        mgr.request_certificate().await?;
        Ok(Json(serde_json::json!({"renewed": true, "forced": true})))
    } else {
        mgr.request_certificate().await?;
        Ok(Json(serde_json::json!({"renewed": true})))
    }
}

// ---------------------------------------------------------------------------
// POST /acme/revoke
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct AcmeRevokeQuery {
    pub reason: Option<String>,
}

pub async fn acme_revoke(
    State(state): State<AdminState>,
    Query(q): Query<AcmeRevokeQuery>,
) -> Result<Json<serde_json::Value>, GatewayError> {
    let mgr = require_acme(&state)?;
    let reason = q.reason.as_deref().and_then(|r| match r {
        "key_compromise" => Some(instant_acme::RevocationReason::KeyCompromise),
        "affiliation_changed" => Some(instant_acme::RevocationReason::AffiliationChanged),
        "superseded" => Some(instant_acme::RevocationReason::Superseded),
        "cessation_of_operation" => Some(instant_acme::RevocationReason::CessationOfOperation),
        _ => None,
    });
    mgr.revoke_certificate(reason).await?;
    Ok(Json(serde_json::json!({"revoked": true})))
}

// ---------------------------------------------------------------------------
// GET /quota
// ---------------------------------------------------------------------------

/// Return the current period's quota snapshot: plan tier, calls allowed,
/// calls remaining, calls rejected. Useful for dashboards and billing UIs.
pub async fn quota_snapshot(
    State(state): State<AdminState>,
) -> Result<impl IntoResponse, GatewayError> {
    Ok(Json(state.quota.snapshot()))
}

// ============================================================================
// ACME challenge admin endpoints
// ============================================================================
//
// External ACME orchestrators (lego, custom clients, certbot --manual)
// drive validation by:
//   1. Asking the ACME directory for a token + key authorization.
//   2. POSTing it to the running gateway here.
//   3. Telling the directory "go validate now".
//   4. DELETEing the challenge to free the slot.
//
// HTTP-01 and TLS-ALPN-01 share the shape; the only difference is the
// resource the validator hits (a path on port 80 vs. a TLS handshake on
// port 443). The same gateway process serves both without any restart.

#[derive(Debug, Deserialize)]
pub struct Http01ChallengeReq {
    pub token: String,
    pub key_authorization: String,
}

#[derive(Debug, Deserialize)]
pub struct TlsAlpnChallengeReq {
    pub domain: String,
    pub key_authorization: String,
}

#[derive(Debug, Serialize)]
pub struct AcmeStatus {
    pub enabled: bool,
    pub has_cert: bool,
    pub http01_challenges: usize,
    pub tls_alpn01_challenges: usize,
}

fn require_acme(state: &AdminState) -> Result<&Arc<crate::proxy::acme::AcmeManager>, GatewayError> {
    state
        .acme
        .as_ref()
        .ok_or_else(|| GatewayError::Internal("acme is disabled in gateway config".into()))
}

pub async fn set_http01_challenge(
    State(state): State<AdminState>,
    Json(req): Json<Http01ChallengeReq>,
) -> Result<StatusCode, GatewayError> {
    let mgr = require_acme(&state)?;
    if req.token.is_empty() || req.key_authorization.is_empty() {
        return Err(GatewayError::ConfigError(
            "token and key_authorization are required".into(),
        ));
    }
    mgr.set_challenge(req.token, req.key_authorization);
    Ok(StatusCode::CREATED)
}

pub async fn clear_http01_challenge(
    State(state): State<AdminState>,
    Path(token): Path<String>,
) -> Result<StatusCode, GatewayError> {
    let mgr = require_acme(&state)?;
    mgr.clear_challenge(&token);
    Ok(StatusCode::NO_CONTENT)
}

pub async fn set_tls_alpn_challenge(
    State(state): State<AdminState>,
    Json(req): Json<TlsAlpnChallengeReq>,
) -> Result<StatusCode, GatewayError> {
    let mgr = require_acme(&state)?;
    if req.domain.is_empty() || req.key_authorization.is_empty() {
        return Err(GatewayError::ConfigError(
            "domain and key_authorization are required".into(),
        ));
    }
    mgr.set_tls_alpn_challenge(req.domain, req.key_authorization)?;
    Ok(StatusCode::CREATED)
}

pub async fn clear_tls_alpn_challenge(
    State(state): State<AdminState>,
    Path(domain): Path<String>,
) -> Result<StatusCode, GatewayError> {
    let mgr = require_acme(&state)?;
    mgr.clear_tls_alpn_challenge(&domain);
    Ok(StatusCode::NO_CONTENT)
}

pub async fn acme_status(State(state): State<AdminState>) -> Json<AcmeStatus> {
    match state.acme.as_ref() {
        None => Json(AcmeStatus {
            enabled: false,
            has_cert: false,
            http01_challenges: 0,
            tls_alpn01_challenges: 0,
        }),
        Some(mgr) => Json(AcmeStatus {
            enabled: true,
            has_cert: mgr.has_cert().await,
            http01_challenges: mgr.http01_challenge_count(),
            tls_alpn01_challenges: mgr.tls_alpn_challenge_count(),
        }),
    }
}
