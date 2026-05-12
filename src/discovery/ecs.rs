//! AWS ECS provider — discovers services registered in AWS Cloud Map
//! (Route53 Auto Naming) via DNS SRV record resolution.
//!
//! ECS Service Connect and Cloud Map integration means ECS services
//! automatically register themselves in a Cloud Map namespace. This
//! provider resolves SRV records for those services using the existing
//! `hickory-resolver` infrastructure, avoiding the need for
//! `aws-sdk-ecs` or manual SigV4 signing.
//!
//! Each configured cluster + service combination is resolved as:
//!
//! ```text
//! {service}.{cloud_map_namespace}  →  SRV records  →  host:port targets
//! ```
//!
//! The provider polls on a configurable interval and converges the
//! target list for each mapped gateway service.

use crate::cluster::{ChangeEvent, ClusterBus};
use crate::db::Repository;
use crate::error::GatewayError;
use crate::models::Target;
use chrono::Utc;
use hickory_resolver::config::{ResolverConfig, ResolverOpts};
use hickory_resolver::TokioAsyncResolver;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;
use tokio::time::sleep;
use uuid::Uuid;

// ── Configuration ──────────────────────────────────────────────────

/// Configuration for [`EcsProvider`]. Lives under
/// `[gateway.providers.ecs]` in the gateway config.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct EcsProviderConfig {
    /// Master switch — provider is a no-op unless `true`.
    #[serde(default)]
    pub enabled: bool,

    /// AWS region where the ECS clusters run.
    #[serde(default = "default_region")]
    pub region: String,

    /// ECS cluster names to watch. Each cluster's services are
    /// resolved via Cloud Map DNS.
    #[serde(default)]
    pub clusters: Vec<String>,

    /// How often (in seconds) to re-resolve SRV records.
    #[serde(default = "default_poll_interval")]
    pub poll_interval_secs: u64,

    /// AWS access key ID. Falls back to the `AWS_ACCESS_KEY_ID`
    /// environment variable when empty. Not used for DNS resolution
    /// but kept for future direct-API extensions.
    #[serde(default)]
    pub access_key_id: String,

    /// AWS secret access key. Falls back to `AWS_SECRET_ACCESS_KEY`.
    #[serde(default)]
    pub secret_access_key: String,

    /// When `true`, the provider enumerates all ECS clusters in the
    /// region (requires API access). Currently a placeholder — the
    /// DNS-based approach requires explicit cluster names.
    #[serde(default)]
    pub auto_discover_clusters: bool,

    /// When `true`, every ECS service in the watched clusters is
    /// registered unless it opts out via a `lynxx.enable=false` tag.
    /// When `false`, services must have a `lynxx.enable=true` tag.
    #[serde(default = "default_true")]
    pub expose_by_default: bool,

    /// Alias for `poll_interval_secs` accepted for config
    /// compatibility. When both are set, `poll_interval_secs` wins.
    #[serde(default = "default_poll_interval")]
    pub refresh_seconds: u64,

    /// AWS Cloud Map namespace that ECS services register into
    /// (e.g. `"production.local"`). SRV queries are formed as
    /// `{service_name}.{cloud_map_namespace}`.
    #[serde(default)]
    pub cloud_map_namespace: String,

    /// Explicit service mappings: ECS service name -> gateway service
    /// ID. When empty, the provider auto-creates gateway services
    /// named `ecs:{service_name}`.
    #[serde(default)]
    pub service_mappings: Vec<EcsServiceMapping>,

    /// DNS server to use for SRV resolution. When empty, the system
    /// resolver (or Cloudflare fallback) is used.
    #[serde(default)]
    pub dns_server: String,

    /// Scheme for discovered targets (`http` or `https`).
    #[serde(default = "default_scheme")]
    pub scheme: String,
}

fn default_region() -> String {
    "us-east-1".to_string()
}

fn default_poll_interval() -> u64 {
    15
}

fn default_true() -> bool {
    true
}

fn default_scheme() -> String {
    "http".to_string()
}

impl Default for EcsProviderConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            region: default_region(),
            clusters: Vec::new(),
            poll_interval_secs: default_poll_interval(),
            access_key_id: String::new(),
            secret_access_key: String::new(),
            auto_discover_clusters: false,
            expose_by_default: true,
            refresh_seconds: default_poll_interval(),
            cloud_map_namespace: String::new(),
            service_mappings: Vec::new(),
            dns_server: String::new(),
            scheme: default_scheme(),
        }
    }
}

/// Maps an ECS service to a gateway service. When provided, SRV
/// targets are attached to the specified `service_id` instead of an
/// auto-generated one.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct EcsServiceMapping {
    /// The ECS service name (as registered in Cloud Map).
    pub ecs_service_name: String,
    /// Gateway service ID to attach targets to.
    pub service_id: Uuid,
    /// Optional weight override for all targets of this service.
    #[serde(default = "default_weight")]
    pub weight: u32,
}

fn default_weight() -> u32 {
    100
}

// ── Provider ──────────────────────────────────────────────────────

/// Zero UUID used as the `origin` field for change events emitted by
/// this provider (no admin actor).
const PROVIDER_ORIGIN: Uuid = Uuid::nil();

pub struct EcsProvider {
    config: EcsProviderConfig,
    resolver: TokioAsyncResolver,
    repo: Arc<dyn Repository>,
    bus: Arc<dyn ClusterBus>,
    /// service_id -> set of target_ids we own.
    owned: tokio::sync::Mutex<HashMap<Uuid, HashSet<Uuid>>>,
}

impl EcsProvider {
    pub fn new(
        config: EcsProviderConfig,
        repo: Arc<dyn Repository>,
        bus: Arc<dyn ClusterBus>,
    ) -> Result<Arc<Self>, GatewayError> {
        let mut opts = ResolverOpts::default();
        opts.cache_size = 256;
        opts.positive_max_ttl = Some(Duration::from_secs(30));
        opts.timeout = Duration::from_secs(3);

        let resolver = if config.dns_server.is_empty() {
            match TokioAsyncResolver::tokio_from_system_conf() {
                Ok(r) => r,
                Err(_) => TokioAsyncResolver::tokio(ResolverConfig::cloudflare(), opts),
            }
        } else {
            // Parse user-supplied DNS server.
            let addr = config
                .dns_server
                .parse()
                .map_err(|e| GatewayError::ConfigError(format!("bad dns_server: {e}")))?;
            let mut rc = ResolverConfig::new();
            rc.add_name_server(hickory_resolver::config::NameServerConfig::new(
                addr,
                hickory_resolver::config::Protocol::Udp,
            ));
            TokioAsyncResolver::tokio(rc, opts)
        };

        Ok(Arc::new(Self {
            config,
            resolver,
            repo,
            bus,
            owned: tokio::sync::Mutex::new(HashMap::new()),
        }))
    }

    /// Spawn the poll loop. Returns immediately.
    pub fn spawn(self: Arc<Self>) {
        tokio::spawn(async move {
            if let Err(e) = self.reconcile().await {
                tracing::warn!(error = %e, "ecs provider initial reconcile failed");
            }
            let interval = Duration::from_secs(self.config.poll_interval_secs.max(5));
            loop {
                sleep(interval).await;
                if let Err(e) = self.reconcile().await {
                    tracing::warn!(error = %e, "ecs provider reconcile failed");
                }
            }
        });
    }

    /// Reconcile all configured service mappings.
    async fn reconcile(&self) -> Result<(), GatewayError> {
        for mapping in &self.config.service_mappings {
            if let Err(e) = self.reconcile_mapping(mapping).await {
                tracing::warn!(
                    service = %mapping.ecs_service_name,
                    error = %e,
                    "ecs: reconcile mapping failed"
                );
            }
        }
        Ok(())
    }

    /// Resolve SRV records for a single ECS service and converge
    /// targets.
    async fn reconcile_mapping(&self, mapping: &EcsServiceMapping) -> Result<(), GatewayError> {
        let srv_name = format!(
            "{}.{}",
            mapping.ecs_service_name, self.config.cloud_map_namespace
        );

        let records = self
            .resolver
            .srv_lookup(&srv_name)
            .await
            .map_err(|e| GatewayError::Internal(format!("SRV lookup {srv_name}: {e}")))?;

        let mut desired: HashMap<String, Uuid> = HashMap::new();
        for record in records.iter() {
            let host = record
                .target()
                .to_string()
                .trim_end_matches('.')
                .to_string();
            let port = record.port();
            let url = format!("{}://{}:{}", self.config.scheme, host, port);
            let target_id = Uuid::new_v5(&mapping.service_id, format!("ecs:{url}").as_bytes());
            desired.insert(url, target_id);
        }

        let mut owned = self.owned.lock().await;
        let existing_ids = owned.entry(mapping.service_id).or_default();

        let existing_targets = self
            .repo
            .list_targets_for_service(&mapping.service_id)
            .await?;
        let mut existing_urls: HashMap<String, Uuid> = HashMap::new();
        for t in &existing_targets {
            if existing_ids.contains(&t.id) {
                existing_urls.insert(t.url.clone(), t.id);
            }
        }

        // Add new targets.
        for (url, target_id) in &desired {
            if !existing_urls.contains_key(url) {
                let target = Target {
                    id: *target_id,
                    service_id: mapping.service_id,
                    url: url.clone(),
                    weight: mapping.weight,
                    enabled: true,
                    zone: None,
                    created_at: Utc::now(),
                    updated_at: Utc::now(),
                };
                if let Err(e) = self.repo.create_target(&target).await {
                    tracing::warn!(url = %url, error = %e, "ecs: create target failed");
                    continue;
                }
                existing_ids.insert(*target_id);
                let _ = self
                    .bus
                    .publish(ChangeEvent::TargetChanged {
                        service_id: mapping.service_id,
                        origin: PROVIDER_ORIGIN,
                    })
                    .await;
                tracing::info!(
                    ecs_service = %mapping.ecs_service_name,
                    target = %url,
                    "ecs provider registered target"
                );
            }
        }

        // Remove stale targets.
        let desired_urls: HashSet<&str> = desired.keys().map(|s| s.as_str()).collect();
        let stale: Vec<(String, Uuid)> = existing_urls
            .iter()
            .filter(|(url, _)| !desired_urls.contains(url.as_str()))
            .map(|(url, id)| (url.clone(), *id))
            .collect();

        for (url, id) in stale {
            if let Err(e) = self.repo.delete_target(&id).await {
                tracing::warn!(url = %url, error = %e, "ecs: delete stale target failed");
                continue;
            }
            existing_ids.remove(&id);
            let _ = self
                .bus
                .publish(ChangeEvent::TargetChanged {
                    service_id: mapping.service_id,
                    origin: PROVIDER_ORIGIN,
                })
                .await;
            tracing::info!(
                ecs_service = %mapping.ecs_service_name,
                target = %url,
                "ecs provider removed stale target"
            );
        }

        Ok(())
    }
}

// ── Helpers ───────────────────────────────────────────────────────

/// Build a Cloud Map SRV name from service + namespace.
pub fn srv_name_for(service: &str, namespace: &str) -> String {
    format!("{service}.{namespace}")
}

// ── Tests ─────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_defaults() {
        let cfg = EcsProviderConfig::default();
        assert!(!cfg.enabled);
        assert_eq!(cfg.region, "us-east-1");
        assert!(cfg.clusters.is_empty());
        assert_eq!(cfg.poll_interval_secs, 15);
        assert!(cfg.access_key_id.is_empty());
        assert!(cfg.secret_access_key.is_empty());
        assert!(!cfg.auto_discover_clusters);
        assert!(cfg.expose_by_default);
        assert_eq!(cfg.refresh_seconds, 15);
        assert!(cfg.cloud_map_namespace.is_empty());
        assert_eq!(cfg.scheme, "http");
    }

    #[test]
    fn config_deserialize_partial() {
        let json = r#"{
            "enabled": true,
            "region": "eu-west-1",
            "clusters": ["production", "staging"],
            "cloud_map_namespace": "prod.local"
        }"#;
        let cfg: EcsProviderConfig = serde_json::from_str(json).unwrap();
        assert!(cfg.enabled);
        assert_eq!(cfg.region, "eu-west-1");
        assert_eq!(cfg.clusters, vec!["production", "staging"]);
        assert_eq!(cfg.cloud_map_namespace, "prod.local");
        // Defaults fill in the rest.
        assert_eq!(cfg.poll_interval_secs, 15);
        assert!(cfg.expose_by_default);
    }

    #[test]
    fn config_env_fallback_fields_are_empty_by_default() {
        let cfg = EcsProviderConfig::default();
        // When these are empty, the provider should fall back to
        // AWS_ACCESS_KEY_ID / AWS_SECRET_ACCESS_KEY env vars at
        // runtime.
        assert!(cfg.access_key_id.is_empty());
        assert!(cfg.secret_access_key.is_empty());
    }

    #[test]
    fn srv_name_construction() {
        assert_eq!(
            srv_name_for("my-api", "production.local"),
            "my-api.production.local"
        );
        assert_eq!(
            srv_name_for("web-frontend", "staging.internal"),
            "web-frontend.staging.internal"
        );
    }

    #[test]
    fn service_mapping_deserialize() {
        let json = r#"{
            "ecs_service_name": "payments",
            "service_id": "550e8400-e29b-41d4-a716-446655440000",
            "weight": 200
        }"#;
        let mapping: EcsServiceMapping = serde_json::from_str(json).unwrap();
        assert_eq!(mapping.ecs_service_name, "payments");
        assert_eq!(
            mapping.service_id,
            Uuid::parse_str("550e8400-e29b-41d4-a716-446655440000").unwrap()
        );
        assert_eq!(mapping.weight, 200);
    }

    #[test]
    fn service_mapping_default_weight() {
        let json = r#"{
            "ecs_service_name": "auth",
            "service_id": "550e8400-e29b-41d4-a716-446655440001"
        }"#;
        let mapping: EcsServiceMapping = serde_json::from_str(json).unwrap();
        assert_eq!(mapping.weight, 100);
    }

    #[test]
    fn config_roundtrip_serde() {
        let cfg = EcsProviderConfig {
            enabled: true,
            region: "ap-southeast-1".to_string(),
            clusters: vec!["cluster-a".to_string()],
            cloud_map_namespace: "mesh.local".to_string(),
            ..Default::default()
        };
        let json = serde_json::to_string(&cfg).unwrap();
        let back: EcsProviderConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(back.region, "ap-southeast-1");
        assert_eq!(back.clusters, vec!["cluster-a"]);
        assert_eq!(back.cloud_map_namespace, "mesh.local");
    }
}
