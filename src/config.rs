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
    }
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
