#![allow(dead_code)]

mod admin;
mod cluster;
mod config;
mod db;
mod error;
mod health;
mod metrics;
mod models;
mod plugins;
mod proxy;
mod reload;

use crate::cluster::ChangeEvent;
use crate::config::load_config;
use crate::db::Repository;
use crate::health::{HealthConfig, HealthRegistry};
use crate::plugins::PluginExecutor;
use crate::proxy::handler::{proxy_handler, ProxyState};
use crate::proxy::loadbalancer::{LbStrategy, LoadBalancerRegistry};
use crate::proxy::router::{RouteTable, SharedRouteTable};
use std::sync::Arc;
use tokio::net::TcpListener;
use tracing_subscriber::EnvFilter;
use uuid::Uuid;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| {
            EnvFilter::new("lynx_gateway=info,tower_http=info")
        }))
        .init();

    let metrics_handle = metrics::install();
    metrics::describe();

    let config = load_config("config.toml")?;

    tracing::info!(
        proxy_port = config.gateway.proxy_port,
        admin_port = config.gateway.admin_port,
        "starting lynx gateway"
    );

    let repo: Arc<dyn Repository> = crate::db::connect(&config.gateway.database_url).await?;
    tracing::info!(database_url = %config.gateway.database_url, "database ready");

    if config.gateway.seed_from_config && repo.is_empty().await? {
        tracing::info!("database empty; seeding from config.toml");
        repo.seed_from_config(&config).await?;
    }

    let route_table = Arc::new(RouteTable::new());
    let (svc_count, route_count) = route_table.load_from_repo(repo.as_ref()).await?;
    let plugin_count = repo.list_plugins().await?.len();
    let consumer_count = repo.list_consumers().await?.len();
    tracing::info!(
        services = svc_count,
        routes = route_count,
        plugins = plugin_count,
        consumers = consumer_count,
        "loaded persisted state"
    );

    let plugin_exec = Arc::new(PluginExecutor::new());
    plugin_exec.rebuild(&repo).await?;
    tracing::info!("plugin executor ready");

    let http_client = reqwest::Client::new();
    let grpc_client = crate::proxy::grpc::build_grpc_client();

    let health_config = HealthConfig {
        failure_threshold: config.gateway.health_circuit_failure_threshold,
        success_threshold: config.gateway.health_circuit_success_threshold,
        open_duration_ms: config.gateway.health_circuit_open_duration_ms,
    };
    let health = Arc::new(HealthRegistry::with_config(health_config));
    let lb = Arc::new(LoadBalancerRegistry::new());
    for svc in route_table.list_services() {
        health.register(svc.id);
        let targets = repo.list_targets_for_service(&svc.id).await?;
        lb.rebuild_for_service(svc.id, LbStrategy::from_str(&svc.lb_strategy), targets);
    }

    health::checker::spawn_health_checker(
        route_table.clone(),
        health.clone(),
        repo.clone(),
        lb.clone(),
        http_client.clone(),
        config.gateway.health_check_interval_ms,
    );

    let proxy_state = ProxyState {
        route_table: route_table.clone(),
        plugins: plugin_exec.clone(),
        http_client,
        grpc_client,
        health: health.clone(),
        lb: lb.clone(),
    };

    let proxy_app = axum::Router::new()
        .fallback(proxy_handler)
        .with_state(proxy_state);

    metrics::spawn_metrics_collector(
        route_table.clone(),
        repo.clone(),
        health.clone(),
        5_000,
    );

    // ---------- cluster bus ----------
    let node_id = Uuid::new_v4();
    tracing::info!(node_id = %node_id, "cluster node id assigned");
    let bus = repo.cluster_bus();

    let _subscriber_handle = spawn_cluster_subscriber(
        node_id,
        bus.clone(),
        repo.clone(),
        route_table.clone(),
        health.clone(),
        lb.clone(),
        plugin_exec.clone(),
    );

    let reload_handle = reload::spawn_reloader(
        "config.toml".to_string(),
        repo.clone(),
        route_table.clone(),
        health.clone(),
        lb.clone(),
        plugin_exec.clone(),
        bus.clone(),
        node_id,
    );
    reload::install_sighup_handler(reload_handle.clone());

    let admin_app = admin::admin_router(
        route_table,
        repo,
        health,
        lb,
        metrics_handle,
        reload_handle,
        plugin_exec,
        bus,
        node_id,
    );

    let proxy_addr = format!("0.0.0.0:{}", config.gateway.proxy_port);
    let admin_addr = format!("0.0.0.0:{}", config.gateway.admin_port);

    let proxy_listener = TcpListener::bind(&proxy_addr).await?;
    let admin_listener = TcpListener::bind(&admin_addr).await?;

    tracing::info!("proxy listening on {proxy_addr}");
    tracing::info!("admin listening on {admin_addr}");

    tokio::select! {
        result = axum::serve(proxy_listener, proxy_app) => {
            if let Err(e) = result {
                tracing::error!("proxy server error: {e}");
            }
        }
        result = axum::serve(admin_listener, admin_app) => {
            if let Err(e) = result {
                tracing::error!("admin server error: {e}");
            }
        }
    }

    Ok(())
}

/// Spawn a task that subscribes to the cluster bus and refreshes local
/// in-memory state when a remote node mutates the DB. Self-published
/// events are filtered by `node_id` inside the bus implementation.
fn spawn_cluster_subscriber(
    node_id: Uuid,
    bus: Arc<dyn cluster::ClusterBus>,
    repo: Arc<dyn Repository>,
    route_table: SharedRouteTable,
    health: Arc<HealthRegistry>,
    lb: Arc<LoadBalancerRegistry>,
    plugin_exec: Arc<PluginExecutor>,
) -> tokio::task::JoinHandle<()> {
    let handler: cluster::ChangeHandler = Arc::new(move |event: ChangeEvent| {
        let repo = repo.clone();
        let route_table = route_table.clone();
        let health = health.clone();
        let lb = lb.clone();
        let plugin_exec = plugin_exec.clone();
        Box::pin(async move {
            if let Err(e) = apply_remote_event(
                event,
                &repo,
                &route_table,
                &health,
                &lb,
                &plugin_exec,
            )
            .await
            {
                tracing::warn!(error = %e, "failed to apply remote cluster event");
            }
        })
    });
    bus.subscribe(node_id, handler)
}

/// Apply a remote `ChangeEvent` by re-fetching the affected entity from the
/// repository and updating local in-memory state. The DB is the source of
/// truth — events only carry IDs.
async fn apply_remote_event(
    event: ChangeEvent,
    repo: &Arc<dyn Repository>,
    route_table: &SharedRouteTable,
    health: &Arc<HealthRegistry>,
    lb: &Arc<LoadBalancerRegistry>,
    plugin_exec: &Arc<PluginExecutor>,
) -> Result<(), error::GatewayError> {
    match event {
        ChangeEvent::ServiceCreated { id, .. } | ChangeEvent::ServiceUpdated { id, .. } => {
            match repo.get_service(&id).await? {
                Some(svc) => {
                    let is_new = route_table.get_service(&id).is_none();
                    route_table.add_service(svc.clone());
                    if is_new {
                        health.register(svc.id);
                    }
                    let targets = repo.list_targets_for_service(&id).await?;
                    lb.rebuild_for_service(svc.id, LbStrategy::from_str(&svc.lb_strategy), targets);
                }
                None => {
                    tracing::warn!(
                        service_id = %id,
                        "remote ServiceCreated/Updated but row missing in DB"
                    );
                }
            }
        }
        ChangeEvent::ServiceDeleted { id, .. } => {
            route_table.remove_service(&id);
            health.unregister(&id);
            lb.remove_service(&id);
        }
        ChangeEvent::RouteCreated { id, .. } | ChangeEvent::RouteUpdated { id, .. } => {
            match repo.get_route(&id).await? {
                Some(route) => {
                    route_table.add_route(route);
                }
                None => {
                    tracing::warn!(
                        route_id = %id,
                        "remote RouteCreated/Updated but row missing in DB"
                    );
                }
            }
        }
        ChangeEvent::RouteDeleted { id, .. } => {
            route_table.remove_route(&id);
        }
        ChangeEvent::TargetChanged { service_id, .. } => {
            match route_table.get_service(&service_id) {
                Some(svc) => {
                    let targets = repo.list_targets_for_service(&service_id).await?;
                    lb.rebuild_for_service(
                        service_id,
                        LbStrategy::from_str(&svc.lb_strategy),
                        targets,
                    );
                }
                None => {
                    tracing::warn!(
                        service_id = %service_id,
                        "remote TargetChanged for unknown service; ignoring"
                    );
                }
            }
        }
        ChangeEvent::PluginChanged { .. } => {
            plugin_exec.rebuild(repo).await?;
        }
        ChangeEvent::ConsumerChanged { .. } => {
            // Consumers are read on-demand by basic-auth/key-auth/hmac-auth
            // plugins; nothing in-memory to refresh.
        }
        ChangeEvent::FullSync { .. } => {
            route_table.load_from_repo(repo.as_ref()).await?;
            for svc in route_table.list_services() {
                health.register(svc.id);
                let targets = repo.list_targets_for_service(&svc.id).await?;
                lb.rebuild_for_service(svc.id, LbStrategy::from_str(&svc.lb_strategy), targets);
            }
            plugin_exec.rebuild(repo).await?;
        }
    }
    Ok(())
}
