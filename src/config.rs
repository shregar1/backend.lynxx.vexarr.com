use serde::Deserialize;

#[derive(Debug, Deserialize, Clone)]
pub struct GatewayConfig {
    #[serde(default = "default_gateway")]
    pub gateway: GatewaySettings,
    #[serde(default)]
    pub services: Vec<ServiceConfig>,
    #[serde(default)]
    pub routes: Vec<RouteConfig>,
    #[serde(default)]
    pub plugins: Vec<PluginConfig>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct GatewaySettings {
    #[serde(default = "default_proxy_port")]
    pub proxy_port: u16,
    #[serde(default = "default_admin_port")]
    pub admin_port: u16,
    #[serde(default = "default_database_url")]
    pub database_url: String,
    #[serde(default)]
    pub seed_from_config: bool,
    #[serde(default = "default_hc_interval")]
    pub health_check_interval_ms: u64,
    #[serde(default = "default_hc_failure_threshold")]
    pub health_circuit_failure_threshold: u32,
    #[serde(default = "default_hc_success_threshold")]
    pub health_circuit_success_threshold: u32,
    #[serde(default = "default_hc_open_duration")]
    pub health_circuit_open_duration_ms: u64,
}

#[derive(Debug, Deserialize, Clone)]
pub struct ServiceConfig {
    pub name: String,
    pub url: String,
    #[serde(default = "default_retries")]
    pub retries: u32,
    #[serde(default = "default_timeout")]
    pub connect_timeout_ms: u64,
}

#[derive(Debug, Deserialize, Clone)]
pub struct RouteConfig {
    pub paths: Vec<String>,
    #[serde(default)]
    pub methods: Vec<String>,
    pub service: String,
    #[serde(default)]
    pub strip_path: bool,
}

#[derive(Debug, Deserialize, Clone)]
pub struct PluginConfig {
    pub name: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub config: toml::Table,
}

impl Default for GatewayConfig {
    fn default() -> Self {
        Self {
            gateway: default_gateway(),
            services: Vec::new(),
            routes: Vec::new(),
            plugins: Vec::new(),
        }
    }
}

fn default_gateway() -> GatewaySettings {
    GatewaySettings {
        proxy_port: 8000,
        admin_port: 8001,
        database_url: default_database_url(),
        seed_from_config: false,
        health_check_interval_ms: default_hc_interval(),
        health_circuit_failure_threshold: default_hc_failure_threshold(),
        health_circuit_success_threshold: default_hc_success_threshold(),
        health_circuit_open_duration_ms: default_hc_open_duration(),
    }
}

fn default_hc_interval() -> u64 {
    10_000
}

fn default_hc_failure_threshold() -> u32 {
    3
}

fn default_hc_success_threshold() -> u32 {
    2
}

fn default_hc_open_duration() -> u64 {
    30_000
}

fn default_database_url() -> String {
    "sqlite://lynxx.db?mode=rwc".to_string()
}

fn default_proxy_port() -> u16 {
    8000
}

fn default_admin_port() -> u16 {
    8001
}

fn default_retries() -> u32 {
    3
}

fn default_timeout() -> u64 {
    5000
}

fn default_true() -> bool {
    true
}

pub fn load_config(path: &str) -> anyhow::Result<GatewayConfig> {
    match std::fs::read_to_string(path) {
        Ok(contents) => {
            let config: GatewayConfig = toml::from_str(&contents)?;
            Ok(config)
        }
        Err(_) => {
            tracing::warn!("config file not found at {path}, using defaults");
            Ok(GatewayConfig::default())
        }
    }
}

/// Apply environment-variable overrides on top of the loaded TOML config.
///
/// TOML remains the primary source of truth; env vars provide a thin overlay
/// for the values most commonly set in container/Helm deployments.
///
/// Recognised vars:
///   * `DATABASE_URL`       overrides `gateway.database_url`
///   * `LYNXX_PROXY_PORT`   overrides `gateway.proxy_port`
///   * `LYNXX_ADMIN_PORT`   overrides `gateway.admin_port`
pub fn apply_env_overrides(config: &mut GatewayConfig) {
    if let Ok(url) = std::env::var("DATABASE_URL") {
        if !url.is_empty() {
            tracing::info!("DATABASE_URL env override applied");
            config.gateway.database_url = url;
        }
    }
    if let Some(port) = parse_port_env("LYNXX_PROXY_PORT") {
        tracing::info!(port, "LYNXX_PROXY_PORT env override applied");
        config.gateway.proxy_port = port;
    }
    if let Some(port) = parse_port_env("LYNXX_ADMIN_PORT") {
        tracing::info!(port, "LYNXX_ADMIN_PORT env override applied");
        config.gateway.admin_port = port;
    }
}

fn parse_port_env(name: &str) -> Option<u16> {
    let raw = std::env::var(name).ok()?;
    match raw.parse::<u16>() {
        Ok(p) => Some(p),
        Err(e) => {
            tracing::warn!(var = name, value = %raw, error = %e, "ignoring invalid port env var");
            None
        }
    }
}
