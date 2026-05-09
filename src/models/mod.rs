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
            created_at: now,
            updated_at: now,
        }
    }
}
