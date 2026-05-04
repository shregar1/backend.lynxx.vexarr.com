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
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

fn default_lb_strategy() -> String {
    "round-robin".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Route {
    pub id: Uuid,
    pub paths: Vec<String>,
    pub methods: Vec<String>,
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
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
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
            created_at: now,
            updated_at: now,
        }
    }
}

impl Route {
    pub fn new(
        paths: Vec<String>,
        methods: Vec<String>,
        service_id: Uuid,
        strip_path: bool,
    ) -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::new_v4(),
            paths,
            methods,
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
