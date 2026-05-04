#![allow(dead_code)]

mod admin;
mod config;
mod error;
mod models;
mod plugins;
mod proxy;

use crate::config::load_config;
use crate::plugins::logging::LoggingPlugin;
use crate::plugins::rate_limit::RateLimitPlugin;
use crate::plugins::cors::CorsPlugin;
use crate::plugins::PluginRegistry;
use crate::proxy::handler::{proxy_handler, ProxyState};
use crate::proxy::router::RouteTable;
use models::{Route, Service};
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

    let config = load_config("config.toml")?;

    tracing::info!(
        proxy_port = config.gateway.proxy_port,
        admin_port = config.gateway.admin_port,
        "starting lynx gateway"
    );

    let route_table = Arc::new(RouteTable::new());

    for svc_cfg in &config.services {
        let service = Service::new(
            svc_cfg.name.clone(),
            svc_cfg.url.clone(),
            svc_cfg.retries,
            svc_cfg.connect_timeout_ms,
        );
        let service_id = service.id;
        route_table.add_service(service);

        for rt_cfg in &config.routes {
            if rt_cfg.service == svc_cfg.name {
                let route = Route::new(
                    rt_cfg.paths.clone(),
                    rt_cfg.methods.clone(),
                    service_id,
                    rt_cfg.strip_path,
                );
                route_table.add_route(route);
            }
        }
    }

    let mut plugin_registry = PluginRegistry::new();

    for plugin_cfg in &config.plugins {
        if !plugin_cfg.enabled {
            continue;
        }
        match plugin_cfg.name.as_str() {
            "rate-limit" => {
                let rps = plugin_cfg
                    .config
                    .get("requests_per_second")
                    .and_then(|v| v.as_float())
                    .or_else(|| {
                        plugin_cfg
                            .config
                            .get("requests_per_second")
                            .and_then(|v| v.as_integer())
                            .map(|i| i as f64)
                    })
                    .unwrap_or(100.0);
                plugin_registry.register(Arc::new(RateLimitPlugin::new(rps)));
            }
            "cors" => {
                let origins = plugin_cfg
                    .config
                    .get("allow_origins")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(|s| s.to_string()))
                            .collect()
                    })
                    .unwrap_or_else(|| vec!["*".to_string()]);
                plugin_registry.register(Arc::new(CorsPlugin::new(origins)));
            }
            "logging" => {
                plugin_registry.register(Arc::new(LoggingPlugin::new()));
            }
            other => {
                tracing::warn!("unknown plugin: {other}, skipping");
            }
        }
    }

    let plugin_registry = Arc::new(plugin_registry);
    let http_client = reqwest::Client::new();

    let proxy_state = ProxyState {
        route_table: route_table.clone(),
        plugins: plugin_registry,
        http_client,
    };

    let proxy_app = axum::Router::new()
        .fallback(proxy_handler)
        .with_state(proxy_state);

    let admin_app = admin::admin_router(route_table);

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
