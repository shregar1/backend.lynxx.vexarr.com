pub mod routes;

use crate::models::{Consumer, PluginInstance};
use crate::proxy::router::SharedRouteTable;
use axum::routing::get;
use axum::Router;
use chrono::Utc;
use dashmap::DashMap;
use routes::AdminState;
use std::sync::Arc;
use uuid::Uuid;

pub fn admin_router(route_table: SharedRouteTable) -> Router {
    let state = AdminState {
        route_table,
        plugins: Arc::new(DashMap::<Uuid, PluginInstance>::new()),
        consumers: Arc::new(DashMap::<Uuid, Consumer>::new()),
        start_time: Utc::now(),
    };

    Router::new()
        .route("/services", get(routes::list_services).post(routes::create_service))
        .route(
            "/services/{id}",
            get(routes::get_service)
                .put(routes::update_service)
                .delete(routes::delete_service),
        )
        .route("/routes", get(routes::list_routes).post(routes::create_route))
        .route(
            "/routes/{id}",
            get(routes::get_route)
                .put(routes::update_route)
                .delete(routes::delete_route),
        )
        .route("/plugins", get(routes::list_plugins).post(routes::create_plugin))
        .route(
            "/consumers",
            get(routes::list_consumers).post(routes::create_consumer),
        )
        .route("/status", get(routes::gateway_status))
        .with_state(state)
}
