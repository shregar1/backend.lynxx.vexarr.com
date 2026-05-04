pub mod routes;

use crate::db::Repository;
use crate::health::HealthRegistry;
use crate::plugins::PluginExecutor;
use crate::proxy::loadbalancer::LoadBalancerRegistry;
use crate::proxy::router::SharedRouteTable;
use crate::reload::ReloadHandle;
use axum::routing::{get, post};
use axum::Router;
use chrono::Utc;
use metrics_exporter_prometheus::PrometheusHandle;
use routes::AdminState;
use std::sync::Arc;

pub fn admin_router(
    route_table: SharedRouteTable,
    repo: Arc<dyn Repository>,
    health: Arc<HealthRegistry>,
    lb: Arc<LoadBalancerRegistry>,
    metrics_handle: PrometheusHandle,
    reload: ReloadHandle,
    plugin_exec: Arc<PluginExecutor>,
) -> Router {
    let state = AdminState {
        route_table,
        repo,
        health,
        lb,
        start_time: Utc::now(),
        metrics_handle,
        reload,
        plugin_exec,
    };

    Router::new()
        .route("/services", get(routes::list_services).post(routes::create_service))
        .route(
            "/services/:id",
            get(routes::get_service)
                .put(routes::update_service)
                .delete(routes::delete_service),
        )
        .route(
            "/services/:id/targets",
            get(routes::list_service_targets).post(routes::create_service_target),
        )
        .route("/targets", get(routes::list_all_targets))
        .route(
            "/targets/:id",
            axum::routing::put(routes::update_target_handler)
                .delete(routes::delete_target_handler),
        )
        .route("/routes", get(routes::list_routes).post(routes::create_route))
        .route(
            "/routes/:id",
            get(routes::get_route)
                .put(routes::update_route)
                .delete(routes::delete_route),
        )
        .route("/plugins", get(routes::list_plugins).post(routes::create_plugin))
        .route(
            "/plugins/:id",
            get(routes::get_plugin)
                .put(routes::update_plugin)
                .delete(routes::delete_plugin),
        )
        .route(
            "/consumers",
            get(routes::list_consumers).post(routes::create_consumer),
        )
        .route("/health", get(routes::list_health))
        .route("/health/:service_id", get(routes::get_health))
        .route("/status", get(routes::gateway_status))
        .route("/metrics", get(routes::metrics))
        .route("/reload", post(routes::reload_handler))
        .with_state(state)
}
