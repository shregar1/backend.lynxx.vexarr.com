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

    /// Admin API TLS. When enabled, the admin port serves HTTPS and
    /// optionally requires client certs for mTLS authentication.
    #[serde(default)]
    pub admin_tls: AdminTlsConfig,

    /// Built-in PKI / internal Certificate Authority. When enabled, the
    /// gateway can issue certificates for internal services.
    #[serde(default)]
    pub pki: PkiConfig,

    /// Certificate storage backend for clustered deployments. Controls
    /// where ACME certs, OCSP responses, and STEK keys are persisted.
    /// Defaults to `"file"` (local disk). Use `"redis"` or `"postgres"`
    /// to share state across multiple gateway nodes.
    #[serde(default)]
    pub cert_store: CertStoreConfig,
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

/// Certificate storage backend configuration. In single-node deployments
/// the default `"file"` backend reads/writes cert material to local disk.
/// Multi-node clusters should use `"redis"` or `"postgres"` so all nodes
/// share ACME certificates, OCSP staples, and STEK rotation keys.
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct CertStoreConfig {
    /// Backend type: `"file"` (default), `"redis"`, `"postgres"`.
    #[serde(default = "default_cert_store_backend")]
    pub backend: String,
    /// Redis URL (only used when `backend = "redis"`).
    /// Example: `redis://127.0.0.1:6379/1`
    #[serde(default)]
    pub redis_url: String,
    /// Directory for the file backend. When empty, falls back to the ACME
    /// `storage_dir` or `~/.lynxx/certs/`.
    #[serde(default)]
    pub file_dir: String,
}

impl Default for CertStoreConfig {
    fn default() -> Self {
        Self {
            backend: default_cert_store_backend(),
            redis_url: String::new(),
            file_dir: String::new(),
        }
    }
}

fn default_cert_store_backend() -> String {
    "file".to_string()
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

    /// DNS-01 challenge provider. When configured, the gateway can
    /// complete DNS-01 challenges for wildcard certs or when ports
    /// 80/443 are not reachable.
    #[serde(default)]
    pub dns01: Dns01Config,

    /// External Account Binding (EAB) credentials. Required by some
    /// commercial CAs (ZeroSSL, BuyPass, Google Trust Services).
    #[serde(default)]
    pub eab_kid: String,
    /// HMAC key for EAB, base64url-encoded.
    #[serde(default)]
    pub eab_hmac_key: String,
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

/// DNS-01 ACME challenge provider configuration. Supports Cloudflare
/// and Route53. When configured, `challenge_type` should include
/// `"dns-01"` or `"auto"`.
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Dns01Config {
    /// Provider name: `"cloudflare"` or `"route53"`.
    #[serde(default)]
    pub provider: String,
    /// Cloudflare API token with Zone:DNS:Edit permission.
    #[serde(default)]
    pub api_token: String,
    /// Cloudflare Zone ID.
    #[serde(default)]
    pub zone_id: String,
    /// Route53 hosted zone ID.
    #[serde(default)]
    pub hosted_zone_id: String,
    /// AWS region (defaults to `us-east-1`). Also reads `AWS_REGION`.
    #[serde(default)]
    pub region: String,
    /// AWS access key ID. Also reads `AWS_ACCESS_KEY_ID`.
    #[serde(default)]
    pub access_key_id: String,
    /// AWS secret access key. Also reads `AWS_SECRET_ACCESS_KEY`.
    #[serde(default)]
    pub secret_access_key: String,
    /// Seconds to wait for DNS propagation. Default 120.
    #[serde(default = "default_dns_propagation_timeout")]
    pub propagation_timeout_secs: u64,
}

impl Default for Dns01Config {
    fn default() -> Self {
        Self {
            provider: String::new(),
            api_token: String::new(),
            zone_id: String::new(),
            hosted_zone_id: String::new(),
            region: String::new(),
            access_key_id: String::new(),
            secret_access_key: String::new(),
            propagation_timeout_secs: default_dns_propagation_timeout(),
        }
    }
}

fn default_dns_propagation_timeout() -> u64 {
    120
}

/// Admin API TLS settings. When enabled, the admin port serves HTTPS
/// and optionally requires client certs (mTLS) for authentication.
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct AdminTlsConfig {
    /// Enable TLS on the admin listener.
    #[serde(default)]
    pub enabled: bool,
    /// Path to the admin server certificate PEM.
    #[serde(default)]
    pub cert_path: String,
    /// Path to the admin server private key PEM.
    #[serde(default)]
    pub key_path: String,
    /// Path to the CA bundle for client certificate verification.
    /// When set, only clients presenting a cert signed by this CA
    /// can access the admin API (mutual TLS).
    #[serde(default)]
    pub client_ca_path: String,
    /// Require client certs (true) or make them optional (false).
    #[serde(default)]
    pub client_cert_required: bool,
}

impl Default for AdminTlsConfig {
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
            dns01: Dns01Config::default(),
            eab_kid: String::new(),
            eab_hmac_key: String::new(),
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
    /// Minimum TLS version. Defaults to `"1.2"`. Accepted: `"1.2"`, `"1.3"`.
    #[serde(default = "default_tls_min_version")]
    pub min_version: String,
    /// Maximum TLS version. Defaults to `"1.3"`.
    #[serde(default = "default_tls_max_version")]
    pub max_version: String,
    /// Named groups / curves for key exchange. Defaults to all supported
    /// by the ring provider: `["x25519", "secp256r1", "secp384r1"]`.
    ///
    /// Post-quantum key exchange (`x25519mlkem768`) requires switching to
    /// the `aws-lc-rs` crypto provider in a future release.
    #[serde(default)]
    pub curves: Vec<String>,
}

impl Default for TlsSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            cert_path: String::new(),
            key_path: String::new(),
            client_ca_path: String::new(),
            client_cert_required: false,
            min_version: default_tls_min_version(),
            max_version: default_tls_max_version(),
            curves: Vec::new(),
        }
    }
}

fn default_tls_min_version() -> String {
    "1.2".to_string()
}

fn default_tls_max_version() -> String {
    "1.3".to_string()
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
        admin_tls: AdminTlsConfig::default(),
        pki: PkiConfig::default(),
        cert_store: CertStoreConfig::default(),
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

/// Process `import` directives in a config file. Lines matching
/// `import = "path"` or `import = ["path1", "path2"]` in TOML, or
/// `import: path` in YAML, are resolved relative to the parent
/// directory of the main config file and merged via deep overlay.
fn resolve_imports(contents: &str, base_dir: &std::path::Path, format: ConfigFormat) -> anyhow::Result<String> {
    match format {
        ConfigFormat::Toml => {
            let mut table: toml::Value = toml::from_str(contents)?;
            if let Some(imports) = table.as_table_mut().and_then(|t| t.remove("import")) {
                let paths = match imports {
                    toml::Value::String(s) => vec![s],
                    toml::Value::Array(arr) => arr
                        .into_iter()
                        .filter_map(|v| v.as_str().map(|s| s.to_string()))
                        .collect(),
                    _ => vec![],
                };
                for p in &paths {
                    let resolved = resolve_import_path(p, base_dir);
                    let snippet = std::fs::read_to_string(&resolved)
                        .map_err(|e| anyhow::anyhow!("import {}: {e}", resolved.display()))?;
                    tracing::info!("importing config snippet from {}", resolved.display());
                    let snippet_val: toml::Value = toml::from_str(&snippet)?;
                    merge_toml(&mut table, &snippet_val);
                }
            }
            Ok(toml::to_string_pretty(&table)?)
        }
        ConfigFormat::Yaml => {
            let mut val: serde_yaml::Value = serde_yaml::from_str(contents)?;
            if let Some(imports) = val.as_mapping_mut().and_then(|m| m.remove(&serde_yaml::Value::String("import".into()))) {
                let paths: Vec<String> = match imports {
                    serde_yaml::Value::String(s) => vec![s],
                    serde_yaml::Value::Sequence(seq) => seq
                        .into_iter()
                        .filter_map(|v| v.as_str().map(|s| s.to_string()))
                        .collect(),
                    _ => vec![],
                };
                for p in &paths {
                    let resolved = resolve_import_path(p, base_dir);
                    let snippet = std::fs::read_to_string(&resolved)
                        .map_err(|e| anyhow::anyhow!("import {}: {e}", resolved.display()))?;
                    tracing::info!("importing config snippet from {}", resolved.display());
                    let snippet_val: serde_yaml::Value = serde_yaml::from_str(&snippet)?;
                    merge_yaml(&mut val, &snippet_val);
                }
            }
            Ok(serde_yaml::to_string(&val)?)
        }
        ConfigFormat::Json => {
            let clean = strip_jsonc_comments(contents);
            let mut val: serde_json::Value = serde_json::from_str(&clean)?;
            if let Some(imports) = val.as_object_mut().and_then(|m| m.remove("import")) {
                let paths: Vec<String> = match imports {
                    serde_json::Value::String(s) => vec![s],
                    serde_json::Value::Array(arr) => arr
                        .into_iter()
                        .filter_map(|v| v.as_str().map(|s| s.to_string()))
                        .collect(),
                    _ => vec![],
                };
                for p in &paths {
                    let resolved = resolve_import_path(p, base_dir);
                    let snippet = std::fs::read_to_string(&resolved)
                        .map_err(|e| anyhow::anyhow!("import {}: {e}", resolved.display()))?;
                    tracing::info!("importing config snippet from {}", resolved.display());
                    let snippet_clean = strip_jsonc_comments(&snippet);
                    let snippet_val: serde_json::Value = serde_json::from_str(&snippet_clean)?;
                    merge_json(&mut val, &snippet_val);
                }
            }
            Ok(serde_json::to_string_pretty(&val)?)
        }
    }
}

fn resolve_import_path(path: &str, base_dir: &std::path::Path) -> std::path::PathBuf {
    let p = std::path::Path::new(path);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        base_dir.join(p)
    }
}

fn merge_toml(base: &mut toml::Value, overlay: &toml::Value) {
    match (base, overlay) {
        (toml::Value::Table(b), toml::Value::Table(o)) => {
            for (k, v) in o {
                let entry = b.entry(k.clone()).or_insert_with(|| toml::Value::Boolean(false));
                merge_toml(entry, v);
            }
        }
        (toml::Value::Array(b), toml::Value::Array(o)) => {
            b.extend(o.iter().cloned());
        }
        (base, overlay) => {
            *base = overlay.clone();
        }
    }
}

fn merge_yaml(base: &mut serde_yaml::Value, overlay: &serde_yaml::Value) {
    match (base, overlay) {
        (serde_yaml::Value::Mapping(b), serde_yaml::Value::Mapping(o)) => {
            for (k, v) in o {
                let entry = b.entry(k.clone()).or_insert(serde_yaml::Value::Null);
                merge_yaml(entry, v);
            }
        }
        (serde_yaml::Value::Sequence(b), serde_yaml::Value::Sequence(o)) => {
            b.extend(o.iter().cloned());
        }
        (base, overlay) => {
            *base = overlay.clone();
        }
    }
}

fn merge_json(base: &mut serde_json::Value, overlay: &serde_json::Value) {
    match (base, overlay) {
        (serde_json::Value::Object(b), serde_json::Value::Object(o)) => {
            for (k, v) in o {
                let entry = b.entry(k.clone()).or_insert(serde_json::Value::Null);
                merge_json(entry, v);
            }
        }
        (serde_json::Value::Array(b), serde_json::Value::Array(o)) => {
            b.extend(o.iter().cloned());
        }
        (base, overlay) => {
            *base = overlay.clone();
        }
    }
}

pub fn load_config(path: &str) -> anyhow::Result<GatewayConfig> {
    match std::fs::read_to_string(path) {
        Ok(contents) => {
            let base_dir = std::path::Path::new(path)
                .parent()
                .unwrap_or(std::path::Path::new("."));

            if let Some(fmt) = detect_format(path) {
                tracing::info!("loading config as {:?} from {path}", fmt);
                let processed = resolve_imports(&contents, base_dir, fmt)?;
                parse_config(&processed, fmt)
            } else {
                tracing::info!(
                    "no recognised extension for {path}, probing TOML → YAML → JSON"
                );
                // Try each format; imports only work if format detection succeeds.
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
