use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Serialize, Clone)]
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

#[derive(Debug, Deserialize, Serialize, Clone, Default)]
pub struct LicenseSettings {
    /// API key issued by the licence server (e.g. `lnx_live_…`). May
    /// also be set via `LYNXX_API_KEY` env var, which wins on conflict.
    #[serde(default)]
    pub api_key: String,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
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
    /// generating self-signed certs for dev, serving HTTP-01 +
    /// TLS-ALPN-01 challenges, and hot-reloading on renewal. See
    /// [`AcmeConfig`].
    #[serde(default)]
    pub acme: AcmeConfig,

    /// External service-discovery providers (Docker today, Kubernetes
    /// next). Each provider is a no-op until its `enabled` flag is
    /// flipped. The Docker provider watches the Docker socket and
    /// registers services + routes from container labels; see
    /// `crate::discovery::docker`.
    #[serde(default)]
    pub providers: ProvidersConfig,

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

    /// Local zone/region identifier for locality-aware load balancing.
    /// When set, the load balancer prioritizes targets in the same zone.
    #[serde(default)]
    pub local_zone: Option<String>,

    /// Built-in PKI / internal Certificate Authority. When enabled, the
    /// gateway can issue certificates for internal services.
    #[serde(default)]
    pub pki: PkiConfig,
}

/// Built-in PKI configuration. When `enabled = true` the gateway
/// generates an internal root + intermediate CA on first start and
/// can issue leaf certificates for internal services — similar to
/// Caddy's built-in ACME server or Vault's PKI secrets engine.
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct PkiConfig {
    /// Master switch. Defaults to `false`.
    #[serde(default)]
    pub enabled: bool,
    /// Where to store the CA keypairs. Defaults to `~/.lynxx/pki/`.
    #[serde(default)]
    pub storage_dir: String,
    /// Root CA common name.
    #[serde(default = "default_pki_cn")]
    pub root_cn: String,
    /// Default cert validity in days for issued leaf certs.
    #[serde(default = "default_pki_validity")]
    pub default_validity_days: u32,
}

impl Default for PkiConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            storage_dir: String::new(),
            root_cn: default_pki_cn(),
            default_validity_days: default_pki_validity(),
        }
    }
}

fn default_pki_cn() -> String {
    "Lynxx Internal CA".to_string()
}

fn default_pki_validity() -> u32 {
    90
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
#[derive(Debug, Deserialize, Serialize, Clone)]
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

    /// ACME challenge type: `"http-01"`, `"tls-alpn-01"`, or `"auto"`.
    /// `auto` tries TLS-ALPN-01 first and falls back to HTTP-01.
    /// Defaults to `"auto"`.
    #[serde(default = "default_acme_challenge_type")]
    pub challenge_type: String,

    /// Fallback ACME directory URLs. If the primary `directory_url` fails,
    /// each fallback is tried in order. Defaults to ZeroSSL.
    #[serde(default = "default_acme_fallback_urls")]
    pub fallback_directory_urls: Vec<String>,

    /// On-Demand TLS: provision certificates during the TLS handshake
    /// for previously-unknown domains. When enabled, a TLS connection
    /// for an uncached domain triggers a background ACME order while a
    /// temporary self-signed cert is served for the initial handshake.
    #[serde(default)]
    pub on_demand: OnDemandTlsConfig,
}

/// On-Demand TLS settings. When enabled, the gateway provisions ACME
/// certificates dynamically during the TLS handshake for domains it
/// doesn't already have a cert for. A temporary self-signed cert is
/// returned for the first connection while the real cert is obtained.
///
/// To prevent abuse (attackers flooding with random domains to exhaust
/// ACME rate limits), configure `allowed_domains` or `ask_url`.
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct OnDemandTlsConfig {
    /// Master switch. Defaults to `false`.
    #[serde(default)]
    pub enabled: bool,
    /// Optional list of allowed domains. Empty = allow all.
    #[serde(default)]
    pub allowed_domains: Vec<String>,
    /// Optional HTTP endpoint to validate domain before issuing.
    /// `GET {ask_url}?domain={domain}` -- 200 = allow, anything else = deny.
    #[serde(default)]
    pub ask_url: String,
    /// Max certificates to issue per minute (rate limit).
    #[serde(default = "default_on_demand_rate")]
    pub max_certs_per_minute: u32,
}

impl Default for OnDemandTlsConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            allowed_domains: Vec::new(),
            ask_url: String::new(),
            max_certs_per_minute: default_on_demand_rate(),
        }
    }
}

fn default_on_demand_rate() -> u32 {
    10
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
            challenge_type: default_acme_challenge_type(),
            fallback_directory_urls: default_acme_fallback_urls(),
            on_demand: OnDemandTlsConfig::default(),
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

fn default_acme_challenge_type() -> String {
    "auto".to_string()
}

fn default_acme_fallback_urls() -> Vec<String> {
    vec!["https://acme.zerossl.com/v2/DV90".to_string()]
}

/// External service-discovery providers. Today only `docker` is
/// implemented; future providers (Kubernetes, Consul, Nomad) plug in
/// here with their own config blocks.
#[derive(Debug, Deserialize, Serialize, Clone, Default)]
pub struct ProvidersConfig {
    #[serde(default)]
    pub docker: crate::discovery::docker::DockerProviderConfig,
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
#[derive(Debug, Deserialize, Serialize, Clone)]
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

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ServiceConfig {
    pub name: String,
    #[serde(default)]
    pub url: String,
    #[serde(default = "default_retries")]
    pub retries: u32,
    #[serde(default = "default_timeout")]
    pub connect_timeout_ms: u64,
    #[serde(default)]
    pub service_type: Option<String>,
    #[serde(default)]
    pub document_root: Option<String>,
    #[serde(default)]
    pub index_file: Option<String>,
    #[serde(default)]
    pub spa_fallback: Option<bool>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
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

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct PluginConfig {
    pub name: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub config: serde_json::Value,
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
        providers: ProvidersConfig::default(),
        prewarm_interval_secs: default_prewarm_interval(),
        prewarm_concurrency: default_prewarm_concurrency(),
        auth_required: false,
        recordings_directory: None,
        plan_tier: default_plan_tier(),
        local_zone: None,
        pki: PkiConfig::default(),
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

/// Detected config file format based on extension.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConfigFormat {
    Toml,
    Yaml,
    Json,
}

/// Strip single-line (`//`) and block (`/* */`) comments from JSONC content,
/// producing valid JSON. Handles comments inside string literals correctly.
fn strip_jsonc_comments(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let chars: Vec<char> = input.chars().collect();
    let len = chars.len();
    let mut i = 0;
    let mut in_string = false;

    while i < len {
        if in_string {
            out.push(chars[i]);
            if chars[i] == '\\' && i + 1 < len {
                out.push(chars[i + 1]);
                i += 2;
            } else {
                if chars[i] == '"' {
                    in_string = false;
                }
                i += 1;
            }
        } else if chars[i] == '"' {
            in_string = true;
            out.push(chars[i]);
            i += 1;
        } else if i + 1 < len && chars[i] == '/' && chars[i + 1] == '/' {
            // Single-line comment — skip to end of line.
            i += 2;
            while i < len && chars[i] != '\n' {
                i += 1;
            }
        } else if i + 1 < len && chars[i] == '/' && chars[i + 1] == '*' {
            // Block comment — skip to closing `*/`.
            i += 2;
            while i + 1 < len && !(chars[i] == '*' && chars[i + 1] == '/') {
                i += 1;
            }
            i += 2; // skip `*/`
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    out
}

/// Detect config format from file extension.
fn detect_format(path: &str) -> Option<ConfigFormat> {
    let ext = std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase());
    match ext.as_deref() {
        Some("toml") => Some(ConfigFormat::Toml),
        Some("yaml" | "yml") => Some(ConfigFormat::Yaml),
        Some("json" | "jsonc") => Some(ConfigFormat::Json),
        _ => None,
    }
}

/// Parse a config string in the given format.
fn parse_config(contents: &str, format: ConfigFormat) -> anyhow::Result<GatewayConfig> {
    match format {
        ConfigFormat::Toml => Ok(toml::from_str(contents)?),
        ConfigFormat::Yaml => Ok(serde_yaml::from_str(contents)?),
        ConfigFormat::Json => {
            let clean = strip_jsonc_comments(contents);
            Ok(serde_json::from_str(&clean)?)
        }
    }
}

pub fn load_config(path: &str) -> anyhow::Result<GatewayConfig> {
    match std::fs::read_to_string(path) {
        Ok(contents) => {
            if let Some(fmt) = detect_format(path) {
                tracing::info!("loading config as {:?} from {path}", fmt);
                parse_config(&contents, fmt)
            } else {
                // Unknown or missing extension — try each format in order.
                tracing::info!(
                    "no recognised extension for {path}, probing TOML → YAML → JSON"
                );
                parse_config(&contents, ConfigFormat::Toml)
                    .or_else(|_| parse_config(&contents, ConfigFormat::Yaml))
                    .or_else(|_| parse_config(&contents, ConfigFormat::Json))
                    .map_err(|e| {
                        anyhow::anyhow!(
                            "failed to parse {path} as TOML, YAML, or JSON: {e}"
                        )
                    })
            }
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
