use crate::error::GatewayError;
use crate::models::{Consumer, PluginInstance, Route, Service};
use crate::proxy::router::SharedRouteTable;
use axum::extract::{Path, State};
use axum::Json;
use chrono::Utc;
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use uuid::Uuid;

#[derive(Clone)]
pub struct AdminState {
    pub route_table: SharedRouteTable,
    pub plugins: Arc<DashMap<Uuid, PluginInstance>>,
    pub consumers: Arc<DashMap<Uuid, Consumer>>,
    pub start_time: chrono::DateTime<Utc>,
}

#[derive(Deserialize)]
pub struct CreateService {
    pub name: String,
    pub url: String,
    #[serde(default = "default_retries")]
    pub retries: u32,
    #[serde(default = "default_timeout")]
    pub connect_timeout_ms: u64,
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
    pub service_id: Uuid,
    #[serde(default)]
    pub strip_path: bool,
}

#[derive(Deserialize)]
pub struct CreatePlugin {
    pub name: String,
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
    pub uptime_seconds: i64,
    pub services_count: usize,
    pub routes_count: usize,
    pub plugins_count: usize,
    pub consumers_count: usize,
}

pub async fn list_services(
    State(state): State<AdminState>,
) -> Json<Vec<Service>> {
    Json(state.route_table.list_services())
}

pub async fn create_service(
    State(state): State<AdminState>,
    Json(input): Json<CreateService>,
) -> Result<Json<Service>, GatewayError> {
    let service = Service::new(input.name, input.url, input.retries, input.connect_timeout_ms);
    let id = state.route_table.add_service(service.clone());
    tracing::info!(id = %id, "service created");
    Ok(Json(service))
}

pub async fn get_service(
    State(state): State<AdminState>,
    Path(id): Path<Uuid>,
) -> Result<Json<Service>, GatewayError> {
    state
        .route_table
        .get_service(&id)
        .map(Json)
        .ok_or_else(|| GatewayError::NotFound(format!("service {id}")))
}

pub async fn update_service(
    State(state): State<AdminState>,
    Path(id): Path<Uuid>,
    Json(input): Json<CreateService>,
) -> Result<Json<Service>, GatewayError> {
    let now = Utc::now();
    let existing = state
        .route_table
        .get_service(&id)
        .ok_or_else(|| GatewayError::NotFound(format!("service {id}")))?;

    let updated = Service {
        id,
        name: input.name,
        url: input.url,
        retries: input.retries,
        connect_timeout_ms: input.connect_timeout_ms,
        created_at: existing.created_at,
        updated_at: now,
    };

    state.route_table.update_service(&id, updated.clone());
    Ok(Json(updated))
}

pub async fn delete_service(
    State(state): State<AdminState>,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, GatewayError> {
    if state.route_table.remove_service(&id) {
        Ok(Json(serde_json::json!({"deleted": true})))
    } else {
        Err(GatewayError::NotFound(format!("service {id}")))
    }
}

pub async fn list_routes(
    State(state): State<AdminState>,
) -> Json<Vec<Route>> {
    Json(state.route_table.list_routes())
}

pub async fn create_route(
    State(state): State<AdminState>,
    Json(input): Json<CreateRoute>,
) -> Result<Json<Route>, GatewayError> {
    if state.route_table.get_service(&input.service_id).is_none() {
        return Err(GatewayError::BadRequest(format!(
            "service {} not found",
            input.service_id
        )));
    }

    let route = Route::new(input.paths, input.methods, input.service_id, input.strip_path);
    let id = state.route_table.add_route(route.clone());
    tracing::info!(id = %id, "route created");
    Ok(Json(route))
}

pub async fn get_route(
    State(state): State<AdminState>,
    Path(id): Path<Uuid>,
) -> Result<Json<Route>, GatewayError> {
    state
        .route_table
        .get_route(&id)
        .map(Json)
        .ok_or_else(|| GatewayError::NotFound(format!("route {id}")))
}

pub async fn update_route(
    State(state): State<AdminState>,
    Path(id): Path<Uuid>,
    Json(input): Json<CreateRoute>,
) -> Result<Json<Route>, GatewayError> {
    let now = Utc::now();
    let existing = state
        .route_table
        .get_route(&id)
        .ok_or_else(|| GatewayError::NotFound(format!("route {id}")))?;

    let updated = Route {
        id,
        paths: input.paths,
        methods: input.methods,
        service_id: input.service_id,
        strip_path: input.strip_path,
        created_at: existing.created_at,
        updated_at: now,
    };

    state.route_table.update_route(&id, updated.clone());
    Ok(Json(updated))
}

pub async fn delete_route(
    State(state): State<AdminState>,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, GatewayError> {
    if state.route_table.remove_route(&id) {
        Ok(Json(serde_json::json!({"deleted": true})))
    } else {
        Err(GatewayError::NotFound(format!("route {id}")))
    }
}

pub async fn list_plugins(
    State(state): State<AdminState>,
) -> Json<Vec<PluginInstance>> {
    let plugins: Vec<PluginInstance> = state.plugins.iter().map(|p| p.clone()).collect();
    Json(plugins)
}

pub async fn create_plugin(
    State(state): State<AdminState>,
    Json(input): Json<CreatePlugin>,
) -> Json<PluginInstance> {
    let now = Utc::now();
    let plugin = PluginInstance {
        id: Uuid::new_v4(),
        name: input.name,
        enabled: input.enabled,
        config: input.config,
        created_at: now,
        updated_at: now,
    };
    state.plugins.insert(plugin.id, plugin.clone());
    Json(plugin)
}

pub async fn list_consumers(
    State(state): State<AdminState>,
) -> Json<Vec<Consumer>> {
    let consumers: Vec<Consumer> = state.consumers.iter().map(|c| c.clone()).collect();
    Json(consumers)
}

pub async fn create_consumer(
    State(state): State<AdminState>,
    Json(input): Json<CreateConsumer>,
) -> Json<Consumer> {
    let consumer = Consumer::new(input.username, input.custom_id);
    state.consumers.insert(consumer.id, consumer.clone());
    Json(consumer)
}

pub async fn gateway_status(
    State(state): State<AdminState>,
) -> Json<StatusResponse> {
    let uptime = Utc::now()
        .signed_duration_since(state.start_time)
        .num_seconds();

    Json(StatusResponse {
        status: "running".into(),
        version: env!("CARGO_PKG_VERSION").into(),
        uptime_seconds: uptime,
        services_count: state.route_table.list_services().len(),
        routes_count: state.route_table.list_routes().len(),
        plugins_count: state.plugins.len(),
        consumers_count: state.consumers.len(),
    })
}
