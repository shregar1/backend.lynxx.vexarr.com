use crate::cluster::{ChangeEvent, ClusterBus};
use crate::db::Repository;
use crate::error::GatewayError;
use crate::health::{HealthRegistry, ServiceHealth};
use crate::models::{Consumer, PluginInstance, Route, Service, Target};
use crate::plugins::PluginExecutor;
use crate::proxy::loadbalancer::{LbStrategy, LoadBalancerRegistry};
use crate::proxy::router::SharedRouteTable;
use crate::reload::{ReloadHandle, ReloadResult};
use axum::extract::{Path, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use chrono::Utc;
use metrics_exporter_prometheus::PrometheusHandle;
use serde::{Deserialize, Serialize};
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
}

/// Publish a ChangeEvent on the cluster bus, logging but never failing the
/// admin call if the bus is unhappy. We've already committed to the DB; the
/// remote nodes will eventually reconcile via the next event or restart.
async fn publish_event(state: &AdminState, event: ChangeEvent) {
    if let Err(e) = state.bus.publish(event).await {
        tracing::warn!(error = %e, "failed to publish cluster event");
    }
}

pub async fn reload_handler(
    State(state): State<AdminState>,
) -> Result<Json<ReloadResult>, GatewayError> {
    let result = state.reload.trigger().await?;
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
    pub url: String,
    #[serde(default = "default_retries")]
    pub retries: u32,
    #[serde(default = "default_timeout")]
    pub connect_timeout_ms: u64,
    #[serde(default)]
    pub lb_strategy: Option<String>,
}

#[derive(Deserialize)]
pub struct CreateTarget {
    pub url: String,
    #[serde(default = "default_weight")]
    pub weight: u32,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

#[derive(Deserialize)]
pub struct UpdateTarget {
    pub url: Option<String>,
    pub weight: Option<u32>,
    pub enabled: Option<bool>,
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
    pub service_id: Uuid,
    #[serde(default)]
    pub strip_path: bool,
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
    state.repo.create_service(&service).await?;
    state.route_table.add_service(service.clone());
    state.health.register(service.id);
    state.lb.rebuild_for_service(
        service.id,
        LbStrategy::from_str(&service.lb_strategy),
        Vec::new(),
    );
    tracing::info!(id = %service.id, "service created");
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
        created_at: existing.created_at,
        updated_at: now,
    };

    state.repo.update_service(&id, &updated).await?;
    state.route_table.update_service(&id, updated.clone());

    if strategy_changed {
        let targets = state.repo.list_targets_for_service(&id).await?;
        state
            .lb
            .rebuild_for_service(id, LbStrategy::from_str(&updated.lb_strategy), targets);
    }

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
) -> Result<Json<serde_json::Value>, GatewayError> {
    let deleted = state.repo.delete_service(&id).await?;
    if deleted {
        state.route_table.remove_service(&id);
        state.health.unregister(&id);
        state.lb.remove_service(&id);
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
) -> Result<Json<ServiceHealth>, GatewayError> {
    state
        .health
        .get(&id)
        .map(Json)
        .ok_or_else(|| GatewayError::NotFound(format!("health for service {id}")))
}

pub async fn list_routes(State(state): State<AdminState>) -> Json<Vec<Route>> {
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

    let route = Route::new(
        input.paths,
        input.methods,
        input.service_id,
        input.strip_path,
    );
    state.repo.create_route(&route).await?;
    state.route_table.add_route(route.clone());
    tracing::info!(id = %route.id, "route created");
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

    state.repo.update_route(&id, &updated).await?;
    state.route_table.update_route(&id, updated.clone());
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

pub async fn delete_route(
    State(state): State<AdminState>,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, GatewayError> {
    let deleted = state.repo.delete_route(&id).await?;
    if deleted {
        state.route_table.remove_route(&id);
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
) -> Result<Json<PluginInstance>, GatewayError> {
    state
        .repo
        .get_plugin(&id)
        .await?
        .map(Json)
        .ok_or_else(|| GatewayError::NotFound(format!("plugin {id}")))
}

pub async fn update_plugin(
    State(state): State<AdminState>,
    Path(id): Path<Uuid>,
    Json(input): Json<CreatePlugin>,
) -> Result<Json<PluginInstance>, GatewayError> {
    let existing = state
        .repo
        .get_plugin(&id)
        .await?
        .ok_or_else(|| GatewayError::NotFound(format!("plugin {id}")))?;

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
) -> Result<Json<serde_json::Value>, GatewayError> {
    let deleted = state.repo.delete_plugin(&id).await?;
    if deleted {
        state.plugin_exec.rebuild(&state.repo).await?;
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
    Json(input): Json<CreateConsumer>,
) -> Result<Json<Consumer>, GatewayError> {
    let consumer = Consumer::new(input.username, input.custom_id);
    state.repo.create_consumer(&consumer).await?;
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
    Json(input): Json<CreateTarget>,
) -> Result<Json<Target>, GatewayError> {
    if state.route_table.get_service(&service_id).is_none() {
        return Err(GatewayError::NotFound(format!("service {service_id}")));
    }
    let mut target = Target::new(service_id, input.url, input.weight);
    target.enabled = input.enabled;
    state.repo.create_target(&target).await?;
    rebuild_lb(&state, service_id).await?;
    tracing::info!(target_id = %target.id, service_id = %service_id, "target created");
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
    Json(input): Json<UpdateTarget>,
) -> Result<Json<Target>, GatewayError> {
    let existing = state
        .repo
        .get_target(&id)
        .await?
        .ok_or_else(|| GatewayError::NotFound(format!("target {id}")))?;

    let updated = Target {
        id: existing.id,
        service_id: existing.service_id,
        url: input.url.unwrap_or(existing.url),
        weight: input.weight.unwrap_or(existing.weight).max(1),
        enabled: input.enabled.unwrap_or(existing.enabled),
        created_at: existing.created_at,
        updated_at: Utc::now(),
    };

    state.repo.update_target(&id, &updated).await?;
    rebuild_lb(&state, updated.service_id).await?;
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
) -> Result<Json<serde_json::Value>, GatewayError> {
    let existing = state
        .repo
        .get_target(&id)
        .await?
        .ok_or_else(|| GatewayError::NotFound(format!("target {id}")))?;
    let deleted = state.repo.delete_target(&id).await?;
    if deleted {
        rebuild_lb(&state, existing.service_id).await?;
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
        uptime_seconds: uptime,
        services_count: state.route_table.list_services().len(),
        routes_count: state.route_table.list_routes().len(),
        plugins_count,
        consumers_count,
    }))
}
