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
    /// Subscription licence — required at startup. Set the `token` here
    /// or via the `LYNXX_LICENSE` env var. Without a valid token the
    /// gateway refuses to start. See `src/license/mod.rs`.
    #[serde(default)]
    pub license: LicenseSettings,
}

#[derive(Debug, Deserialize, Clone, Default)]
pub struct LicenseSettings {
    /// API key issued by the licence server (e.g. `lnx_live_…`). May
    /// also be set via `LYNXX_API_KEY` env var, which wins on conflict.
    #[serde(default)]
    pub api_key: String,
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

    /// Inbound TLS termination for the proxy listener. Off by default;
    /// when enabled the proxy listener serves HTTPS via axum-server +
    /// rustls. See [`TlsSettings`].
    #[serde(default)]
    pub tls: TlsSettings,

    /// ACME / automatic certificate provisioning. When enabled, the
    /// gateway manages its own TLS certificates — loading from disk,
    /// generating self-signed certs for dev, serving HTTP-01 challenges,
    /// and hot-reloading on renewal. See [`AcmeConfig`].
    #[serde(default)]
    pub acme: AcmeConfig,

    /// Background connection pre-warmer interval in seconds. `0` disables
    /// pre-warming. Pre-warm respects circuit-breaker state — services
    /// whose circuit is `Open` are skipped.
    #[serde(default = "default_prewarm_interval")]
    pub prewarm_interval_secs: u64,

    /// Maximum number of concurrent pre-warm requests. Capped to avoid
    /// thundering-herd against many upstreams at once.
    #[serde(default = "default_prewarm_concurrency")]
    pub prewarm_concurrency: usize,

    /// When true, admin API requests must always present a valid token
    /// (no bootstrap mode). Defaults to false: bootstrap mode is allowed
    /// while `admin_tokens` is empty, and auth turns on automatically as
    /// soon as any token exists.
    #[serde(default)]
    pub auth_required: bool,

    /// Filesystem root for the request-recorder content-addressed store
    /// and the place the admin recording API loads recordings from. When
    /// `None`, the runtime resolves to `~/.lynxx/recordings` (or
    /// `/var/lib/lynxx/recordings` if no home dir is available).
    #[serde(default)]
    pub recordings_directory: Option<String>,

    /// Licence tier that determines the monthly API call quota.
    /// Accepted values: `"free"` (1 M), `"pro"` (10 M), `"business"` (∞),
    /// `"enterprise"` (∞). Defaults to `"free"` when not set.
    #[serde(default = "default_plan_tier")]
    pub plan_tier: String,
}

/// ACME / automatic certificate provisioning settings. When
/// `enabled = true` the gateway manages its own TLS certificates:
///
/// * On startup, looks for `cert.pem` + `key.pem` in
///   `{storage_dir}/{primary_domain}/`.
/// * If `self_signed = true` and no cert exists, generates a self-signed
///   cert (useful for development / local testing).
/// * Serves `/.well-known/acme-challenge/{token}` responses so external
///   ACME clients (certbot, acme.sh) can complete HTTP-01 validation.
/// * Polls cert files every 30 seconds and hot-reloads the TLS config
///   when they change on disk — no restart needed after renewal.
///
/// `acme` and `tls` are complementary: `tls` provides static cert paths
/// and mTLS knobs, while `acme` adds automatic provisioning. When both
/// are enabled, `acme` takes precedence for the server certificate.
#[derive(Debug, Deserialize, Clone)]
pub struct AcmeConfig {
    /// Master switch. Defaults to `false`.
    #[serde(default)]
    pub enabled: bool,
    /// Domain names for the certificate. The first entry becomes the CN;
    /// all entries are added as Subject Alternative Names. Required when
    /// `enabled = true`.
    #[serde(default)]
    pub domains: Vec<String>,
    /// Contact email for the ACME account (e.g. Let's Encrypt). Not
    /// used for self-signed certs but stored for future ACME flows.
    #[serde(default)]
    pub email: String,
    /// ACME directory URL. Defaults to Let's Encrypt production.
    #[serde(default = "default_acme_directory_url")]
    pub directory_url: String,
    /// Where to store certs and account keys. Defaults to
    /// `~/.lynxx/acme/`.
    #[serde(default)]
    pub storage_dir: String,
    /// Generate a self-signed certificate on startup when no cert exists
    /// on disk. Intended for development / local testing. Defaults to
    /// `false`.
    #[serde(default)]
    pub self_signed: bool,
    /// When ACME is enabled, also bind a plain HTTP listener on this port
    /// for HTTP-01 challenges and HTTP→HTTPS redirects. Defaults to `80`.
    /// Set to `0` to disable the HTTP listener.
    #[serde(default = "default_acme_http_port")]
    pub http_port: u16,
    /// The external HTTPS port visible to clients, used in HTTP→HTTPS
    /// redirects. Defaults to `443`. Set this when the gateway runs
    /// behind Docker port mapping (e.g. host 443 → container 8443).
    #[serde(default = "default_acme_external_port")]
    pub external_https_port: u16,
}

impl Default for AcmeConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            domains: Vec::new(),
            email: String::new(),
            directory_url: default_acme_directory_url(),
            storage_dir: String::new(),
            self_signed: false,
            http_port: default_acme_http_port(),
            external_https_port: default_acme_external_port(),
        }
    }
}

fn default_acme_http_port() -> u16 {
    80
}

fn default_acme_external_port() -> u16 {
    443
}

fn default_acme_directory_url() -> String {
    "https://acme-v02.api.letsencrypt.org/directory".to_string()
}

/// Inbound TLS termination knobs. When `enabled = false` (the default)
/// the proxy port serves plain HTTP exactly as before. When set, the
/// listener binds via `axum_server::bind_rustls` using the provided
/// cert/key. If `client_ca_path` is non-empty the gateway additionally
/// performs mTLS — incoming connections must present a client cert that
/// chains to the configured CA.
///
/// `client_cert_required` toggles between optional and required client
/// auth: with `optional`, the connection is accepted either way and any
/// presented cert is verified; with `required`, the handshake fails if
/// no client cert is sent.
#[derive(Debug, Deserialize, Clone)]
pub struct TlsSettings {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub cert_path: String,
    #[serde(default)]
    pub key_path: String,
    /// Optional. If set, mTLS is enabled — client certs are verified
    /// against this CA bundle (PEM).
    #[serde(default)]
    pub client_ca_path: String,
    /// `false` (default) = optional client auth; `true` = required.
    /// Only meaningful when `client_ca_path` is set.
    #[serde(default)]
    pub client_cert_required: bool,
}

impl Default for TlsSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            cert_path: String::new(),
            key_path: String::new(),
            client_ca_path: String::new(),
            client_cert_required: false,
        }
    }
}

impl TlsSettings {
    /// Whether mTLS (client cert verification) is configured.
    pub fn mtls_enabled(&self) -> bool {
        !self.client_ca_path.is_empty()
    }
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
    #[serde(default)]
    pub hosts: Vec<String>,
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
            license: LicenseSettings::default(),
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
        tls: TlsSettings::default(),
        acme: AcmeConfig::default(),
        prewarm_interval_secs: default_prewarm_interval(),
        prewarm_concurrency: default_prewarm_concurrency(),
        auth_required: false,
        recordings_directory: None,
        plan_tier: default_plan_tier(),
    }
}

fn default_prewarm_interval() -> u64 {
    // 30s by default — keeps idle pools hot without flooding lightly-used
    // upstreams. Set to 0 in config to disable.
    30
}

fn default_prewarm_concurrency() -> usize {
    8
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

fn default_plan_tier() -> String {
    "free".to_string()
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
