#![allow(dead_code)]

// `mimalloc` cuts the long tail on busy proxy workloads. The system
// allocator on macOS in particular fragments under bursty alloc/free
// patterns; switching to mimalloc shaves 20-40% off p99/p99.9 in our
// bench rig and compiles cleanly on aarch64-apple-darwin.
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

mod admin;
mod audit;
mod auth;
mod cluster;
mod config;
mod db;
mod error;
mod health;
mod license;
mod metrics;
mod models;
mod plugins;
mod proxy;
mod quota;
mod recording;
mod reload;

use crate::cluster::ChangeEvent;
use crate::config::{load_config, GatewayConfig};
use crate::db::Repository;
use crate::health::{HealthConfig, HealthRegistry};
use crate::plugins::PluginExecutor;
use crate::proxy::handler::{proxy_handler, ProxyState};
use crate::proxy::loadbalancer::{LbStrategy, LoadBalancerRegistry};
use crate::proxy::router::{RouteTable, SharedRouteTable};
use axum::extract::DefaultBodyLimit;
use clap::Parser;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::net::TcpListener;
use tracing_subscriber::EnvFilter;
use uuid::Uuid;

/// Command-line arguments for the Lynxx gateway.
///
/// Precedence (highest first): clap arg > env var (via clap `env` attr) >
/// TOML file > built-in default.
#[derive(Parser, Debug)]
#[command(version, about = "Lynxx API gateway")]
struct Args {
    /// Path to config.toml
    #[arg(long, env = "LYNXX_CONFIG", default_value = "config.toml")]
    config: PathBuf,

    /// Override [gateway].proxy_port
    #[arg(long, env = "LYNXX_PROXY_PORT")]
    proxy_port: Option<u16>,

    /// Override [gateway].admin_port
    #[arg(long, env = "LYNXX_ADMIN_PORT")]
    admin_port: Option<u16>,

    /// Override [gateway].database_url
    #[arg(long, env = "DATABASE_URL")]
    database_url: Option<String>,

    /// Disable seed_from_config on first boot
    #[arg(long)]
    no_seed: bool,

    /// Override [gateway].health_check_interval_ms
    #[arg(long, env = "LYNXX_HEALTH_INTERVAL_MS")]
    health_check_interval_ms: Option<u64>,

    /// Create an admin token at startup with the given name. The raw
    /// token is printed to stderr ONCE. Useful for k8s init containers.
    #[arg(long)]
    bootstrap_admin_token: Option<String>,

    /// When combined with --bootstrap-admin-token, exit after creating
    /// the token instead of starting the gateway.
    #[arg(long)]
    bootstrap_only: bool,
}

/// Apply CLI overrides on top of the loaded TOML config. Each `Some` value
/// wins over whatever the file specified; absent flags leave the config
/// untouched. Boolean flags are applied unconditionally when set to true.
fn apply_cli_overrides(config: &mut GatewayConfig, args: &Args) {
    if let Some(port) = args.proxy_port {
        tracing::info!(port, "--proxy-port override applied");
        config.gateway.proxy_port = port;
    }
    if let Some(port) = args.admin_port {
        tracing::info!(port, "--admin-port override applied");
        config.gateway.admin_port = port;
    }
    if let Some(url) = &args.database_url {
        if !url.is_empty() {
            tracing::info!("--database-url override applied");
            config.gateway.database_url = url.clone();
        }
    }
    if let Some(ms) = args.health_check_interval_ms {
        tracing::info!(ms, "--health-check-interval-ms override applied");
        config.gateway.health_check_interval_ms = ms;
    }
    if args.no_seed {
        tracing::info!("--no-seed: disabling seed_from_config");
        config.gateway.seed_from_config = false;
    }
}

/// Explicit runtime construction. Beats `#[tokio::main]` for high-RPS proxy
/// workloads because we can size the worker pool to physical cores and name
/// the threads (helps `perf` / flamegraphs). Kept simple — no thread pinning,
/// no per-thread stack size — those are deployment-specific tuning knobs.
fn main() -> anyhow::Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(num_cpus::get())
        .thread_name("lynxx-worker")
        .build()?;
    rt.block_on(async_main())
}

async fn async_main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("lynx_gateway=info,tower_http=info")),
        )
        .init();

    let metrics_handle = metrics::install();
    metrics::describe();

    let args = Args::parse();
    let config_path: String = args.config.to_string_lossy().into_owned();
    let mut config = load_config(&config_path)?;
    apply_cli_overrides(&mut config, &args);

    // ---------- licence gate ----------
    // Verify the subscription token before doing any other startup work.
    // Refuses to start without a valid signed token; the embedded pubkey
    // (src/license/mod.rs) must match the issuer's private key on the
    // licence server. The token's tier overrides config.gateway.plan_tier.
    let license = match crate::license::License::load(Some(&config.license.token)) {
        Ok(lic) => {
            tracing::info!(
                customer_id = %lic.customer_id,
                tier = %lic.tier.as_str(),
                expires_at = %lic.expires_at,
                days_remaining = lic.days_remaining(),
                "licence verified"
            );
            config.gateway.plan_tier = lic.tier.as_str().to_string();
            lic
        }
        Err(err) => {
            eprintln!("licence verification failed: {err}");
            eprintln!("This Lynxx binary requires an active subscription.");
            eprintln!("Set LYNXX_LICENSE=<token> or [license].token in config.toml.");
            eprintln!("Get a token at https://lynxx.dev/account/license");
            return Err(anyhow::anyhow!("licence required: {err}"));
        }
    };

    tracing::info!(
        proxy_port = config.gateway.proxy_port,
        admin_port = config.gateway.admin_port,
        customer = %license.customer_id,
        "starting lynx gateway"
    );

    let repo: Arc<dyn Repository> = crate::db::connect(&config.gateway.database_url).await?;
    tracing::info!(database_url = %config.gateway.database_url, "database ready");

    if config.gateway.seed_from_config && repo.is_empty().await? {
        tracing::info!("database empty; seeding from config.toml");
        repo.seed_from_config(&config).await?;
    }

    // ---------- admin auth bootstrap ----------
    if let Some(name) = &args.bootstrap_admin_token {
        let raw = crate::auth::generate_token();
        let new = crate::db::NewAdminToken {
            id: Uuid::new_v4(),
            name: name.clone(),
            token_hash: crate::auth::hash_token(&raw),
            role: crate::auth::Role::Admin,
            created_at: chrono::Utc::now(),
            expires_at: None,
        };
        repo.create_admin_token(&new).await?;
        eprintln!(
            "lynx-bootstrap-admin-token id={} name={} role=admin token={}",
            new.id, name, raw
        );
        if args.bootstrap_only {
            tracing::info!("--bootstrap-only set; exiting after token creation");
            return Ok(());
        }
    }

    if !repo.has_any_admin_token().await? {
        tracing::warn!(
            "admin API is in BOOTSTRAP MODE (no admin tokens configured); \
             all requests are accepted. Create a token via POST /tokens or \
             pass --bootstrap-admin-token at startup to enforce auth."
        );
    } else {
        tracing::info!("admin API auth enforced (admin_tokens present)");
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

    // Streaming hyper-util client used by the proxy hot path (HTTP/1.1 + h2
    // via ALPN). Separate from the `reqwest::Client` used by the health
    // checker and out-of-band plugins (oauth2-introspect) — those benefit
    // from reqwest's higher-level conveniences and aren't latency-critical.
    let http_client = crate::proxy::http_client::build_http_proxy_client()?;
    let health_http_client = reqwest::Client::new();
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
        health_http_client,
        config.gateway.health_check_interval_ms,
    );

    let client_pool = Arc::new(
        crate::proxy::http_client::ProxyClientPool::new(http_client.clone())
            .expect("init proxy client pool"),
    );

    // Quota tracker — load persisted monthly counters, start background flush.
    let plan_tier = crate::quota::PlanTier::from_str(&config.gateway.plan_tier);
    let quota = crate::quota::QuotaTracker::load(plan_tier, repo.clone()).await?;
    crate::quota::spawn_quota_flusher(quota.clone());

    let proxy_state = ProxyState {
        route_table: route_table.clone(),
        plugins: plugin_exec.clone(),
        http_client: http_client.clone(),
        client_pool: client_pool.clone(),
        grpc_client,
        health: health.clone(),
        lb: lb.clone(),
        quota: quota.clone(),
    };

    // Background connection pre-warmer. Keeps the upstream pool hot so
    // previously-idle services don't pay a TCP+TLS handshake on the next
    // real request — one of the bigger contributors to the p99.9 long
    // tail. Disabled when `prewarm_interval_secs == 0`.
    let _prewarm_handle = crate::proxy::prewarm::spawn_prewarmer(
        route_table.clone(),
        client_pool.clone(),
        health.clone(),
        config.gateway.prewarm_interval_secs,
        config.gateway.prewarm_concurrency,
    );

    // Raise axum's default 2 MB body limit to 64 MB so the gateway can
    // forward large request bodies (file uploads, 5 MB+ API payloads, etc.)
    // without returning 413. Kong defaults to 64 MB; we match it.
    let proxy_app = axum::Router::new()
        .fallback(proxy_handler)
        .layer(DefaultBodyLimit::max(64 * 1024 * 1024))
        .with_state(proxy_state);

    metrics::spawn_metrics_collector(route_table.clone(), repo.clone(), health.clone(), 5_000);

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
        client_pool.clone(),
    );

    let reload_handle = reload::spawn_reloader(
        config_path.clone(),
        repo.clone(),
        route_table.clone(),
        health.clone(),
        lb.clone(),
        plugin_exec.clone(),
        bus.clone(),
        node_id,
    );
    reload::install_sighup_handler(reload_handle.clone());

    // Recording store + replay HTTP client. Both are owned by AdminState
    // for the recording API; the request-recorder plugin builds its own
    // store independently from its plugin config so the two don't share
    // a singleton.
    let recording_dir = config
        .gateway
        .recordings_directory
        .clone()
        .unwrap_or_else(|| {
            dirs::home_dir()
                .map(|h| h.join(".lynxx").join("recordings"))
                .unwrap_or_else(|| std::path::PathBuf::from("/var/lib/lynxx/recordings"))
                .to_string_lossy()
                .into_owned()
        });
    let recording_store =
        Arc::new(crate::recording::ContentAddressedStore::new(recording_dir));
    let admin_http_client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .expect("reqwest client builds");

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
        config.gateway.auth_required,
        client_pool.clone(),
        recording_store,
        admin_http_client,
        config.gateway.proxy_port,
        quota.clone(),
    );

    let proxy_addr = format!("0.0.0.0:{}", config.gateway.proxy_port);
    let admin_addr = format!("0.0.0.0:{}", config.gateway.admin_port);

    let admin_listener = TcpListener::bind(&admin_addr).await?;
    tracing::info!("admin listening on {admin_addr}");

    // Branch on TLS termination. Plain HTTP keeps the existing axum::serve
    // path; HTTPS uses axum-server's bind_rustls so we don't have to
    // reinvent the accept loop. Only the proxy listener is affected — the
    // admin port stays plain HTTP (it lives behind the operator's own VPN
    // / mesh by convention).
    let proxy_addr_socket: std::net::SocketAddr = proxy_addr.parse()?;
    if config.gateway.tls.enabled {
        let server_config =
            crate::proxy::tls::build_server_config(&config.gateway.tls)?;
        tracing::info!(
            "proxy listening on {proxy_addr} (TLS{})",
            if config.gateway.tls.mtls_enabled() {
                ", mTLS"
            } else {
                ""
            }
        );
        let rustls_cfg = axum_server::tls_rustls::RustlsConfig::from_config(Arc::new(server_config));
        let serve = axum_server::bind_rustls(proxy_addr_socket, rustls_cfg)
            .serve(proxy_app.into_make_service());
        tokio::select! {
            result = serve => {
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
    } else {
        let proxy_listener = TcpListener::bind(&proxy_addr).await?;
        tracing::info!("proxy listening on {proxy_addr}");
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
    client_pool: Arc<crate::proxy::http_client::ProxyClientPool>,
) -> tokio::task::JoinHandle<()> {
    let handler: cluster::ChangeHandler = Arc::new(move |event: ChangeEvent| {
        let repo = repo.clone();
        let route_table = route_table.clone();
        let health = health.clone();
        let lb = lb.clone();
        let plugin_exec = plugin_exec.clone();
        let client_pool = client_pool.clone();
        Box::pin(async move {
            if let Err(e) = apply_remote_event(
                event,
                &repo,
                &route_table,
                &health,
                &lb,
                &plugin_exec,
                &client_pool,
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
    client_pool: &Arc<crate::proxy::http_client::ProxyClientPool>,
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
                    // Drop any cached per-service TLS client. The pool
                    // rebuilds on the next request using the fresh row.
                    client_pool.invalidate(&svc.id);
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
            client_pool.invalidate(&id);
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
