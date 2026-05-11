use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Service {
    pub id: Uuid,
    pub name: String,
    pub url: String,
    pub retries: u32,
    pub connect_timeout_ms: u64,
    #[serde(default = "default_lb_strategy")]
    pub lb_strategy: String,
    #[serde(default = "default_service_type")]
    pub service_type: String,
    /// Filesystem root for `service_type = "static"`. Lynxx serves files
    /// directly from this directory instead of proxying to an upstream.
    #[serde(default)]
    pub document_root: Option<String>,
    #[serde(default = "default_index_file")]
    pub index_file: String,
    #[serde(default)]
    pub spa_fallback: bool,
    /// Outbound TLS verification toggle for `https://` upstreams. When
    /// `false`, the per-service hyper-util client skips certificate
    /// validation entirely. Defaults to `true` and should stay there
    /// for any production upstream.
    #[serde(default = "default_tls_verify")]
    pub tls_verify: bool,
    /// Optional client certificate (PEM) presented by the gateway when
    /// initiating mTLS to this service. Both this and `tls_client_key_path`
    /// must be set for client auth to take effect.
    #[serde(default)]
    pub tls_client_cert_path: Option<String>,
    /// PEM-encoded private key paired with `tls_client_cert_path`.
    #[serde(default)]
    pub tls_client_key_path: Option<String>,
    /// Additional CA bundle (PEM) to trust when validating the upstream's
    /// server certificate. Useful for private/internal CAs.
    #[serde(default)]
    pub tls_ca_path: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

fn default_service_type() -> String {
    "api".to_string()
}

fn default_index_file() -> String {
    "index.html".to_string()
}

fn default_tls_verify() -> bool {
    true
}

impl Service {
    /// Cache key fingerprint that captures every TLS-relevant field.
    /// Used by `ProxyClientPool` to invalidate cached per-service clients
    /// on update without resorting to full equality checks.
    pub fn tls_fingerprint(&self) -> String {
        format!(
            "{}|{}|{}|{}",
            self.tls_verify as u8,
            self.tls_client_cert_path.as_deref().unwrap_or(""),
            self.tls_client_key_path.as_deref().unwrap_or(""),
            self.tls_ca_path.as_deref().unwrap_or(""),
        )
    }
}

fn default_lb_strategy() -> String {
    "round-robin".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Route {
    pub id: Uuid,
    pub paths: Vec<String>,
    pub methods: Vec<String>,
    #[serde(default)]
    pub hosts: Vec<String>,
    pub service_id: Uuid,
    pub strip_path: bool,
    /// Regex-based URI rewrite: `path_rewrite_pattern` is the regex to
    /// match and `path_rewrite_replacement` is the substitution string.
    /// E.g. pattern=`^/api/v1/(.*)` replacement=`/v1/$1`.
    #[serde(default)]
    pub path_rewrite_pattern: Option<String>,
    #[serde(default)]
    pub path_rewrite_replacement: Option<String>,
    /// When true, the proxy buffers the entire request body before
    /// forwarding to the upstream. Normally bodies stream through.
    #[serde(default)]
    pub buffer_request: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginInstance {
    pub id: Uuid,
    pub name: String,
    #[serde(default)]
    pub service_id: Option<Uuid>,
    #[serde(default)]
    pub route_id: Option<Uuid>,
    pub enabled: bool,
    pub config: serde_json::Value,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Consumer {
    pub id: Uuid,
    pub username: String,
    pub custom_id: Option<String>,
    pub api_key: Option<String>,
    /// Argon2id password hash. When set, basic-auth verifies the supplied
    /// password against this hash instead of comparing against `api_key`.
    #[serde(default)]
    pub password_hash: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Target {
    pub id: Uuid,
    pub service_id: Uuid,
    pub url: String,
    pub weight: u32,
    pub enabled: bool,
    #[serde(default)]
    pub zone: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceFingerprint {
    pub id: Uuid,
    pub customer_id: i64,
    pub hardware_id: String,
    pub name: Option<String>,
    pub os_info: Option<String>,
    pub is_blocked: bool,
    pub last_seen_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeGrant {
    pub id: Uuid,
    pub customer_id: i64,
    pub device_id: Option<Uuid>,
    pub token_id: String,
    pub expires_at: DateTime<Utc>,
    pub revoked_at: Option<DateTime<Utc>>,
    pub last_heartbeat_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiKeyRow {
    pub id: i64,
    pub customer_id: i64,
    pub name: String,
    pub key_hash: String,
    pub key_prefix: String,
    pub scopes: Vec<String>,
}

impl Target {
    pub fn new(service_id: Uuid, url: String, weight: u32) -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::new_v4(),
            service_id,
            url,
            weight: weight.max(1),
            enabled: true,
            zone: None,
            created_at: now,
            updated_at: now,
        }
    }
}

impl Service {
    pub fn new(name: String, url: String, retries: u32, connect_timeout_ms: u64) -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::new_v4(),
            name,
            url,
            retries,
            connect_timeout_ms,
            lb_strategy: default_lb_strategy(),
            service_type: default_service_type(),
            document_root: None,
            index_file: default_index_file(),
            spa_fallback: false,
            tls_verify: true,
            tls_client_cert_path: None,
            tls_client_key_path: None,
            tls_ca_path: None,
            created_at: now,
            updated_at: now,
        }
    }
}

impl Route {
    pub fn new(
        paths: Vec<String>,
        methods: Vec<String>,
        hosts: Vec<String>,
        service_id: Uuid,
        strip_path: bool,
    ) -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::new_v4(),
            paths,
            methods,
            hosts,
            service_id,
            strip_path,
            path_rewrite_pattern: None,
            path_rewrite_replacement: None,
            buffer_request: false,
            created_at: now,
            updated_at: now,
        }
    }
}

impl Consumer {
    pub fn new(username: String, custom_id: Option<String>) -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::new_v4(),
            username,
            custom_id,
            api_key: Some(Uuid::new_v4().to_string()),
            password_hash: None,
            created_at: now,
            updated_at: now,
        }
    }
}
