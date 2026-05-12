//! ZooKeeper provider — discovers services from a ZooKeeper ensemble
//! via an HTTP bridge (ZooNavigator, zkrest, or the ZK Admin Server).
//!
//! Since there's no production ZooKeeper client crate in our dependency
//! tree, this provider takes a pragmatic approach: it polls a
//! configurable HTTP bridge URL that exposes ZK node data as JSON.
//! Popular options include:
//!
//! - **zkrest** — lightweight REST proxy for ZooKeeper
//! - **ZooNavigator** — web-based ZK browser with a REST API
//! - **ZK Admin Server** — built-in HTTP endpoint (port 8089 by default)
//!
//! Each ZK node under `root_path` represents a service. The node's data
//! payload must be a JSON object with at least a `url` field:
//!
//! ```json
//! {
//!   "url": "http://10.0.0.5:8080",
//!   "weight": 100,
//!   "zone": "us-east-1a"
//! }
//! ```
//!
//! The provider creates a `Target` for each child node, keyed by the
//! node path so restarts don't duplicate entries.

use crate::cluster::{ChangeEvent, ClusterBus};
use crate::db::Repository;
use crate::error::GatewayError;
use crate::models::Target;
use chrono::Utc;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;
use tokio::time::sleep;
use uuid::Uuid;

// ── Configuration ──────────────────────────────────────────────────

/// Configuration for [`ZooKeeperProvider`]. Lives under
/// `[gateway.providers.zookeeper]` in the gateway config.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ZooKeeperProviderConfig {
    /// Master switch — provider is a no-op unless `true`.
    #[serde(default)]
    pub enabled: bool,

    /// ZooKeeper ensemble endpoints (for documentation / connection
    /// string display only — actual communication goes through
    /// `http_bridge_url`).
    #[serde(default = "default_endpoints")]
    pub endpoints: Vec<String>,

    /// ZK node path that acts as the service registry root.
    /// Children of this path are treated as service entries.
    #[serde(default = "default_root_path")]
    pub root_path: String,

    /// How often (in seconds) to poll the HTTP bridge for changes.
    #[serde(default = "default_poll_interval")]
    pub poll_interval_secs: u64,

    /// SASL username for authenticated ZK access (forwarded to the
    /// bridge via basic-auth when non-empty).
    #[serde(default)]
    pub username: String,

    /// SASL password.
    #[serde(default)]
    pub password: String,

    /// ZooKeeper connection string in the standard `host:port` CSV
    /// format. Kept for documentation and logging — all I/O goes
    /// through the HTTP bridge.
    #[serde(default)]
    pub connection_string: String,

    /// URL of the HTTP bridge that exposes ZK data as JSON. The
    /// placeholder `{root_path}` is replaced with [`root_path`] at
    /// runtime.
    #[serde(default = "default_http_bridge_url")]
    pub http_bridge_url: String,
}

fn default_endpoints() -> Vec<String> {
    vec!["127.0.0.1:2181".to_string()]
}

fn default_root_path() -> String {
    "/lynxx".to_string()
}

fn default_poll_interval() -> u64 {
    15
}

fn default_http_bridge_url() -> String {
    "http://127.0.0.1:8089/znodes/v1{root_path}?recursive=true".to_string()
}

impl Default for ZooKeeperProviderConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            endpoints: default_endpoints(),
            root_path: default_root_path(),
            poll_interval_secs: default_poll_interval(),
            username: String::new(),
            password: String::new(),
            connection_string: String::new(),
            http_bridge_url: default_http_bridge_url(),
        }
    }
}

// ── ZK bridge response model ──────────────────────────────────────

/// Represents a single ZK node returned by the HTTP bridge.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct ZkNode {
    /// Full path of the node (e.g. `/lynxx/my-service`).
    pub path: String,
    /// Raw data stored in the node — expected to be a JSON string.
    #[serde(default)]
    pub data: Option<String>,
    /// Child nodes (recursive listing).
    #[serde(default)]
    pub children: Vec<ZkNode>,
}

/// The JSON payload stored in each service znode.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct ZkServiceData {
    /// Upstream URL (required).
    pub url: String,
    /// Optional weight for load-balancing.
    #[serde(default = "default_weight")]
    pub weight: u32,
    /// Optional availability zone tag.
    #[serde(default)]
    pub zone: Option<String>,
    /// Optional service name override. When absent, the znode name is
    /// used.
    #[serde(default)]
    pub name: Option<String>,
}

fn default_weight() -> u32 {
    100
}

/// Envelope returned by some HTTP bridge implementations; the actual
/// nodes may be nested under a `children` key.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct ZkBridgeResponse {
    #[serde(default)]
    pub children: Vec<ZkNode>,
    /// Some bridges put the root node data here.
    #[serde(default)]
    pub data: Option<String>,
    #[serde(default)]
    pub path: Option<String>,
}

// ── Provider ──────────────────────────────────────────────────────

/// Zero UUID used as the `origin` field for change events emitted by
/// this provider (no admin actor).
const PROVIDER_ORIGIN: Uuid = Uuid::nil();

pub struct ZooKeeperProvider {
    config: ZooKeeperProviderConfig,
    client: Client,
    repo: Arc<dyn Repository>,
    bus: Arc<dyn ClusterBus>,
    /// Maps `zk_path -> target_id` so we can detect stale entries.
    owned: tokio::sync::Mutex<HashMap<String, OwnedTarget>>,
}

#[derive(Clone, Debug)]
struct OwnedTarget {
    target_id: Uuid,
    service_id: Uuid,
}

impl ZooKeeperProvider {
    pub fn new(
        config: ZooKeeperProviderConfig,
        repo: Arc<dyn Repository>,
        bus: Arc<dyn ClusterBus>,
    ) -> Result<Arc<Self>, GatewayError> {
        let client = Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .map_err(|e| GatewayError::Internal(format!("zookeeper http client: {e}")))?;

        Ok(Arc::new(Self {
            config,
            client,
            repo,
            bus,
            owned: tokio::sync::Mutex::new(HashMap::new()),
        }))
    }

    /// Spawn the poll loop. Returns immediately.
    pub fn spawn(self: Arc<Self>) {
        tokio::spawn(async move {
            if let Err(e) = self.reconcile().await {
                tracing::warn!(error = %e, "zookeeper provider initial reconcile failed");
            }
            let interval = Duration::from_secs(self.config.poll_interval_secs.max(5));
            loop {
                sleep(interval).await;
                if let Err(e) = self.reconcile().await {
                    tracing::warn!(error = %e, "zookeeper provider reconcile failed");
                }
            }
        });
    }

    /// Build the effective bridge URL by substituting `{root_path}`.
    fn bridge_url(&self) -> String {
        self.config
            .http_bridge_url
            .replace("{root_path}", &self.config.root_path)
    }

    /// Fetch the ZK tree from the HTTP bridge and reconcile targets.
    async fn reconcile(&self) -> Result<(), GatewayError> {
        let url = self.bridge_url();

        let mut req = self.client.get(&url);
        if !self.config.username.is_empty() {
            req = req.basic_auth(&self.config.username, Some(&self.config.password));
        }

        let resp = req
            .send()
            .await
            .map_err(|e| GatewayError::Internal(format!("zk bridge GET {url}: {e}")))?;

        if !resp.status().is_success() {
            return Err(GatewayError::Internal(format!(
                "zk bridge returned {}",
                resp.status()
            )));
        }

        let body = resp
            .text()
            .await
            .map_err(|e| GatewayError::Internal(format!("zk bridge body: {e}")))?;

        let nodes = parse_bridge_response(&body)?;
        let entries = extract_service_entries(&nodes);

        let mut owned = self.owned.lock().await;
        let mut seen_paths: HashSet<String> = HashSet::new();

        for entry in &entries {
            seen_paths.insert(entry.path.clone());

            let service_id = deterministic_uuid(&entry.path, "service");
            let target_id = deterministic_uuid(&entry.path, "target");

            let target = Target {
                id: target_id,
                service_id,
                url: entry.data.url.clone(),
                weight: entry.data.weight,
                enabled: true,
                zone: entry.data.zone.clone(),
                created_at: Utc::now(),
                updated_at: Utc::now(),
            };

            let already = owned.contains_key(&entry.path);
            if already {
                if let Err(e) = self.repo.update_target(&target_id, &target).await {
                    tracing::warn!(path = %entry.path, error = %e, "zk: update target failed");
                }
            } else {
                if let Err(e) = self.repo.create_target(&target).await {
                    tracing::warn!(path = %entry.path, error = %e, "zk: create target failed");
                    continue;
                }
                owned.insert(
                    entry.path.clone(),
                    OwnedTarget {
                        target_id,
                        service_id,
                    },
                );
                tracing::info!(
                    path = %entry.path,
                    url = %entry.data.url,
                    "zookeeper provider registered target"
                );
            }

            let _ = self
                .bus
                .publish(ChangeEvent::TargetChanged {
                    service_id,
                    origin: PROVIDER_ORIGIN,
                })
                .await;
        }

        // Remove stale entries.
        let stale: Vec<(String, OwnedTarget)> = owned
            .iter()
            .filter(|(path, _)| !seen_paths.contains(path.as_str()))
            .map(|(path, ot)| (path.clone(), ot.clone()))
            .collect();

        for (path, ot) in stale {
            let _ = self.repo.delete_target(&ot.target_id).await;
            owned.remove(&path);
            let _ = self
                .bus
                .publish(ChangeEvent::TargetChanged {
                    service_id: ot.service_id,
                    origin: PROVIDER_ORIGIN,
                })
                .await;
            tracing::info!(path = %path, "zookeeper provider removed stale target");
        }

        Ok(())
    }
}

// ── Parsing helpers ───────────────────────────────────────────────

#[derive(Debug, Clone)]
pub(crate) struct ServiceEntry {
    pub path: String,
    pub data: ZkServiceData,
}

/// Parse the HTTP bridge JSON response into a flat list of ZK nodes.
pub(crate) fn parse_bridge_response(body: &str) -> Result<Vec<ZkNode>, GatewayError> {
    // Try the envelope format first (`{ "children": [...] }`), then
    // fall back to a bare array.
    if let Ok(envelope) = serde_json::from_str::<ZkBridgeResponse>(body) {
        return Ok(envelope.children);
    }
    if let Ok(nodes) = serde_json::from_str::<Vec<ZkNode>>(body) {
        return Ok(nodes);
    }
    Err(GatewayError::Internal(
        "zk bridge returned unparseable JSON".to_string(),
    ))
}

/// Walk the node tree and extract entries whose `data` field is valid
/// [`ZkServiceData`] JSON.
pub(crate) fn extract_service_entries(nodes: &[ZkNode]) -> Vec<ServiceEntry> {
    let mut out = Vec::new();
    for node in nodes {
        if let Some(data_str) = &node.data {
            if let Ok(svc) = serde_json::from_str::<ZkServiceData>(data_str) {
                out.push(ServiceEntry {
                    path: node.path.clone(),
                    data: svc,
                });
            }
        }
        // Recurse into children.
        out.extend(extract_service_entries(&node.children));
    }
    out
}

/// Deterministic UUID derived from a ZK path + kind so that restarts
/// don't create duplicate rows.
fn deterministic_uuid(path: &str, kind: &str) -> Uuid {
    let mut input = String::with_capacity(path.len() + kind.len() + 12);
    input.push_str("lynxx:zk:");
    input.push_str(kind);
    input.push(':');
    input.push_str(path);
    Uuid::new_v5(&Uuid::NAMESPACE_URL, input.as_bytes())
}

// ── Tests ─────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_defaults() {
        let cfg = ZooKeeperProviderConfig::default();
        assert!(!cfg.enabled);
        assert_eq!(cfg.endpoints, vec!["127.0.0.1:2181"]);
        assert_eq!(cfg.root_path, "/lynxx");
        assert_eq!(cfg.poll_interval_secs, 15);
        assert_eq!(
            cfg.http_bridge_url,
            "http://127.0.0.1:8089/znodes/v1{root_path}?recursive=true"
        );
        assert!(cfg.username.is_empty());
        assert!(cfg.password.is_empty());
        assert!(cfg.connection_string.is_empty());
    }

    #[test]
    fn config_deserialize_partial() {
        let json = r#"{"enabled": true, "root_path": "/services"}"#;
        let cfg: ZooKeeperProviderConfig = serde_json::from_str(json).unwrap();
        assert!(cfg.enabled);
        assert_eq!(cfg.root_path, "/services");
        // Defaults fill in the rest.
        assert_eq!(cfg.poll_interval_secs, 15);
        assert_eq!(cfg.endpoints, vec!["127.0.0.1:2181"]);
    }

    #[test]
    fn parse_bridge_envelope_response() {
        let body = r#"{
            "children": [
                {
                    "path": "/lynxx/svc-a",
                    "data": "{\"url\":\"http://10.0.0.1:8080\",\"weight\":50}",
                    "children": []
                },
                {
                    "path": "/lynxx/svc-b",
                    "data": "{\"url\":\"http://10.0.0.2:9090\"}",
                    "children": []
                }
            ]
        }"#;

        let nodes = parse_bridge_response(body).unwrap();
        assert_eq!(nodes.len(), 2);

        let entries = extract_service_entries(&nodes);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].path, "/lynxx/svc-a");
        assert_eq!(entries[0].data.url, "http://10.0.0.1:8080");
        assert_eq!(entries[0].data.weight, 50);
        assert_eq!(entries[1].path, "/lynxx/svc-b");
        assert_eq!(entries[1].data.url, "http://10.0.0.2:9090");
        assert_eq!(entries[1].data.weight, 100); // default
    }

    #[test]
    fn parse_bridge_array_response() {
        let body = r#"[
            {
                "path": "/lynxx/web",
                "data": "{\"url\":\"http://web:3000\",\"zone\":\"us-east-1a\"}",
                "children": []
            }
        ]"#;

        let nodes = parse_bridge_response(body).unwrap();
        let entries = extract_service_entries(&nodes);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].data.zone.as_deref(), Some("us-east-1a"));
    }

    #[test]
    fn parse_skips_invalid_data_nodes() {
        let body = r#"{
            "children": [
                {
                    "path": "/lynxx/good",
                    "data": "{\"url\":\"http://good:80\"}",
                    "children": []
                },
                {
                    "path": "/lynxx/bad",
                    "data": "not json at all",
                    "children": []
                },
                {
                    "path": "/lynxx/empty",
                    "children": []
                }
            ]
        }"#;

        let nodes = parse_bridge_response(body).unwrap();
        let entries = extract_service_entries(&nodes);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].path, "/lynxx/good");
    }

    #[test]
    fn parse_nested_children() {
        let body = r#"{
            "children": [
                {
                    "path": "/lynxx/group",
                    "children": [
                        {
                            "path": "/lynxx/group/nested-svc",
                            "data": "{\"url\":\"http://nested:5000\"}",
                            "children": []
                        }
                    ]
                }
            ]
        }"#;

        let nodes = parse_bridge_response(body).unwrap();
        let entries = extract_service_entries(&nodes);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].path, "/lynxx/group/nested-svc");
    }

    #[test]
    fn deterministic_uuid_is_stable() {
        let a = deterministic_uuid("/lynxx/svc-a", "target");
        let b = deterministic_uuid("/lynxx/svc-a", "target");
        let c = deterministic_uuid("/lynxx/svc-a", "service");
        assert_eq!(a, b, "same input produces same uuid");
        assert_ne!(a, c, "different kind produces different uuid");
    }

    #[test]
    fn bridge_url_substitution() {
        let cfg = ZooKeeperProviderConfig {
            root_path: "/myapp/services".to_string(),
            http_bridge_url: "http://zk-bridge:8089/znodes/v1{root_path}?recursive=true"
                .to_string(),
            ..Default::default()
        };
        // Simulate what the provider does.
        let url = cfg.http_bridge_url.replace("{root_path}", &cfg.root_path);
        assert_eq!(
            url,
            "http://zk-bridge:8089/znodes/v1/myapp/services?recursive=true"
        );
    }
}
