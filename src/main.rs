#![allow(dead_code)]

mod admin;
mod config;
mod db;
mod error;
mod health;
mod metrics;
mod models;
mod plugins;
mod proxy;
mod reload;

use crate::config::load_config;
use crate::db::Repository;
use crate::health::{HealthConfig, HealthRegistry};
use crate::plugins::PluginExecutor;
use crate::proxy::handler::{proxy_handler, ProxyState};
use crate::proxy::loadbalancer::{LbStrategy, LoadBalancerRegistry};
use crate::proxy::router::RouteTable;
use std::sync::Arc;
use tokio::net::TcpListener;
use tracing_subscriber::EnvFilter;

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

    let repo = Arc::new(Repository::new(&config.gateway.database_url).await?);
    tracing::info!(database_url = %config.gateway.database_url, "database ready");

    if config.gateway.seed_from_config && repo.is_empty().await? {
        tracing::info!("database empty; seeding from config.toml");
        repo.seed_from_config(&config).await?;
    }

    let route_table = Arc::new(RouteTable::new());
    let (svc_count, route_count) = route_table.load_from_repo(&repo).await?;
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

    let reload_handle = reload::spawn_reloader(
        "config.toml".to_string(),
        repo.clone(),
        route_table.clone(),
        health.clone(),
        lb.clone(),
        plugin_exec.clone(),
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
