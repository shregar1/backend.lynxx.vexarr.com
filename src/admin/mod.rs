pub mod config_api;
pub mod profiling;
pub mod routes;

use crate::audit::AuditLog;
use crate::auth::middleware::{extract_token, require_role};
use crate::auth::Role;
use crate::cluster::ClusterBus;
use crate::config::GatewayConfig;
use crate::db::Repository;
use crate::health::HealthRegistry;
use crate::plugins::PluginExecutor;
use crate::proxy::http_client::ProxyClientPool;
use crate::proxy::loadbalancer::LoadBalancerRegistry;
use crate::proxy::router::SharedRouteTable;
use crate::quota::QuotaTracker;
use crate::reload::ReloadHandle;
use axum::extract::Request;
use axum::http::{header, Method, StatusCode, Uri};
use axum::middleware::{self, Next};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{delete, get, post, put};
use axum::Router;
use chrono::Utc;
use metrics_exporter_prometheus::PrometheusHandle;
use routes::AdminState;
use rust_embed::RustEmbed;
use std::sync::Arc;
use tower_http::cors::{Any, CorsLayer};
use uuid::Uuid;

#[derive(RustEmbed)]
#[folder = "../node.dashboard.lynxx.dev/dist/"]
struct DashboardAssets;

#[allow(clippy::too_many_arguments)]
pub fn admin_router(
    route_table: SharedRouteTable,
    repo: Arc<dyn Repository>,
    health: Arc<HealthRegistry>,
    lb: Arc<LoadBalancerRegistry>,
    metrics_handle: PrometheusHandle,
    reload: ReloadHandle,
    plugin_exec: Arc<PluginExecutor>,
    bus: Arc<dyn ClusterBus>,
    node_id: Uuid,
    auth_required: bool,
    client_pool: Arc<ProxyClientPool>,
    recording_store: Arc<crate::recording::ContentAddressedStore>,
    http_client: reqwest::Client,
    proxy_port: u16,
    quota: Arc<QuotaTracker>,
    acme: Option<Arc<crate::proxy::acme::AcmeManager>>,
    license_api_key: String,
    gateway_config: GatewayConfig,
) -> Router {
    let audit = AuditLog::new(repo.clone());
    let state = AdminState {
        route_table,
        repo,
        health,
        lb,
        start_time: Utc::now(),
        metrics_handle,
        reload,
        plugin_exec,
        bus,
        node_id,
        audit,
        auth_required,
        client_pool,
        recording_store,
        http_client,
        proxy_port,
        quota,
        acme,
        license_api_key,
    };

    // Routes that mutate live state require Operator or higher; token
    // management is Admin-only. Reads are Viewer.
    let viewer = require_role(Role::Viewer);
    let operator = require_role(Role::Operator);
    let admin_only = require_role(Role::Admin);

    // ---- read-only (viewer) ----
    let reads = Router::new()
        .route("/services", get(routes::list_services))
        .route("/services/:id", get(routes::get_service))
        .route("/services/:id/targets", get(routes::list_service_targets))
        .route("/targets", get(routes::list_all_targets))
        .route("/routes", get(routes::list_routes))
        .route("/routes/:id", get(routes::get_route))
        .route("/plugins", get(routes::list_plugins))
        .route("/plugins/:id", get(routes::get_plugin))
        .route("/consumers", get(routes::list_consumers))
        .route("/health", get(routes::list_health))
        .route("/healthz", get(routes::healthz))
        .route("/health/:service_id", get(routes::get_health))
        .route("/status", get(routes::gateway_status))
        .route("/metrics", get(routes::metrics))
        .route("/quota", get(routes::quota_snapshot))
        .route("/circuit-breakers", get(routes::list_circuit_breakers))
        .route("/upstreams", get(routes::list_upstreams))
        .route("/debug/pprof/heap", get(profiling::heap_profile))
        .route("/debug/pprof/tasks", get(profiling::tasks_profile))
        .route("/debug/pprof/threads", get(profiling::threads_profile))
        .route("/debug/pprof/uptime", get(profiling::runtime_profile))
        .route("/debug/pprof/stats", get(profiling::combined_profile))
        .route_layer(middleware::from_fn(viewer));

    // ---- mutations (operator) ----
    let mutations = Router::new()
        .route("/services", post(routes::create_service))
        .route(
            "/services/:id",
            put(routes::update_service)
                .patch(routes::patch_service)
                .delete(routes::delete_service),
        )
        .route("/services/:id/targets", post(routes::create_service_target))
        .route(
            "/targets/:id",
            put(routes::update_target_handler).delete(routes::delete_target_handler),
        )
        .route("/routes", post(routes::create_route))
        .route(
            "/routes/:id",
            put(routes::update_route)
                .patch(routes::patch_route)
                .delete(routes::delete_route),
        )
        .route("/plugins", post(routes::create_plugin))
        .route(
            "/plugins/:id",
            put(routes::update_plugin).delete(routes::delete_plugin),
        )
        .route("/consumers", post(routes::create_consumer))
        .route("/consumers/:id", delete(routes::delete_consumer))
        .route("/reload", post(routes::reload_handler))
        // audit read access lives here — operator+ may inspect, viewers may not.
        .route("/audit", get(routes::list_audit))
        .route("/audit/verify", get(routes::verify_audit))
        // recordings (Time-Travel Debug A3)
        .route("/recordings", get(routes::list_recordings))
        .route(
            "/recordings/:id",
            get(routes::get_recording).delete(routes::delete_recording_handler),
        )
        .route("/recordings/:id/replay", post(routes::replay_recording))
        // ACME challenge endpoints — let an external orchestrator
        // (lego, certbot --manual, custom client) push HTTP-01 and
        // TLS-ALPN-01 challenges into the running gateway without
        // restarting it.
        .route(
            "/acme/http-01-challenges",
            post(routes::set_http01_challenge),
        )
        .route(
            "/acme/http-01-challenges/:token",
            axum::routing::delete(routes::clear_http01_challenge),
        )
        .route(
            "/acme/tls-alpn-01-challenges",
            post(routes::set_tls_alpn_challenge),
        )
        .route(
            "/acme/tls-alpn-01-challenges/:domain",
            axum::routing::delete(routes::clear_tls_alpn_challenge),
        )
        .route("/acme/status", get(routes::acme_status))
        .route("/acme/renew", post(routes::acme_renew))
        .route("/acme/revoke", post(routes::acme_revoke))
        .route(
            "/circuit-breakers/:service_id/reset",
            post(routes::reset_circuit_breaker),
        )
        .route_layer(middleware::from_fn(operator));

    // ---- token management (admin) ----
    let tokens = Router::new()
        .route(
            "/tokens",
            get(routes::list_tokens).post(routes::create_token),
        )
        .route(
            "/tokens/:id",
            put(routes::update_token).delete(routes::delete_token),
        )
        .route_layer(middleware::from_fn(admin_only));


    // ---- live config API ----
    let config_store = config_api::ConfigStore::new(gateway_config);
    let config_sub = config_api::config_router(config_store);

    // ---- dashboard serving ----
    let dashboard = Router::new().fallback(dashboard_handler);

    // ---- routes assembly ----
    let api = reads
        .merge(mutations)
        .merge(tokens)
        .layer(middleware::from_fn_with_state(state.clone(), extract_token));

    dashboard
        .merge(api)
        .with_state(state)
        .nest("/admin/v1/config", config_sub)
        .layer(middleware::from_fn(spa_navigation_guard))
        .layer(admin_cors())
}

/// Browser navigation requests (`Accept: text/html`) get the SPA shell
/// instead of hitting API routes like `/services` that would return JSON.
async fn spa_navigation_guard(req: Request, next: Next) -> Response {
    if req.method() == Method::GET {
        let prefers_html = req
            .headers()
            .get(header::ACCEPT)
            .and_then(|v| v.to_str().ok())
            .map(|a| a.contains("text/html"))
            .unwrap_or(false);

        let path = req.uri().path();
        let is_static_asset = path.contains('.') && !path.ends_with(".html");

        if prefers_html && !is_static_asset {
            return index_handler().await.into_response();
        }
    }

    next.run(req).await
}

async fn dashboard_handler(uri: Uri) -> impl IntoResponse {
    let path = uri.path().trim_start_matches('/');

    if path.is_empty() || path == "index.html" {
        return index_handler().await.into_response();
    }

    match DashboardAssets::get(path) {
        Some(content) => {
            let mime = mime_guess::from_path(path).first_or_octet_stream();
            ([(header::CONTENT_TYPE, mime.as_ref())], content.data).into_response()
        }
        None => {
            // Fallback to index.html for SPA routing
            index_handler().await.into_response()
        }
    }
}

async fn index_handler() -> impl IntoResponse {
    match DashboardAssets::get("index.html") {
        Some(content) => Html(content.data).into_response(),
        None => (StatusCode::NOT_FOUND, "index.html not found").into_response(),
    }
}

/// Permissive CORS for the admin API: any origin, common methods + headers.
/// The admin port is meant for trusted control-plane traffic; locking origins
/// down is the operator's responsibility behind a real auth/proxy layer.
fn admin_cors() -> CorsLayer {
    CorsLayer::new()
        .allow_methods([
            Method::GET,
            Method::POST,
            Method::PUT,
            Method::DELETE,
            Method::PATCH,
            Method::OPTIONS,
        ])
        .allow_headers(Any)
        .allow_origin(Any)
        .expose_headers(Any)
}
