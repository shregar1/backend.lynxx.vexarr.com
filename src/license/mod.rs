//! License gating — simple API-key model.
//!
//! The user pastes a short key like `lnx_live_abc123` into config or
//! the `LYNXX_API_KEY` env var. On startup the gateway calls the licence
//! server to verify the key; on success it caches the result locally so
//! subsequent restarts work offline for up to 7 days.
//!
//! ## Verify request
//!
//! `GET ${LYNXX_BASE_URL}/v1/license/verify?key=<api_key>`
//! (default base: `https://api.lynxx.vexarr.com`)
//!
//! ## Verify response (200)
//!
//! ```json
//! { "customer_id": "cus_abc", "tier": "pro", "expires_at": 1759104800 }
//! ```
//!
//! Any non-200 (401 cancelled, 403 invalid, 5xx server error) is fatal
//! on first start. After a successful verify the result is cached at
//! `~/.lynxx/license.cache.json`. On later restarts a network failure
//! is tolerated for up to `OFFLINE_GRACE_DAYS` after the last good
//! verify; past that the gateway refuses to start.
//!
//! ## Dev-mode keys
//!
//! Debug builds accept keys of the form `lnx_dev_<tier>` (e.g.
//! `lnx_dev_pro`) without contacting the server. Release builds reject
//! them — see `try_dev_key` below. This keeps local development cheap
//! without weakening shipped binaries.

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::env;
use std::path::PathBuf;
use std::time::Duration as StdDuration;

use crate::config::{KeyxSettings, LicenseSettings};
use crate::integrations::keyx::{KeyxClient, KeyxConfig, KeyxError};
use crate::quota::PlanTier;

const DEFAULT_LYNXX_BASE_URL: &str = "https://api.lynxx.vexarr.com";

fn resolve_lynxx_base_url() -> String {
    env::var("LYNXX_BASE_URL")
        .or_else(|_| env::var("LYNXX_LICENSE_VERIFY_URL").map(|u| {
            u.trim_end_matches("/v1/license/verify").to_string()
        }))
        .unwrap_or_else(|_| DEFAULT_LYNXX_BASE_URL.to_string())
        .trim_end_matches('/')
        .to_string()
}

fn verify_url() -> String {
    format!("{}/v1/license/verify", resolve_lynxx_base_url())
}

/// Ed25519 public key for verifying signed `/v1/license/verify`
/// responses. Compiled into the binary — a MITM cannot forge a
/// valid signature without the billing server's private key.
const VERIFY_PUBLIC_KEY_HEX: &str = "c43513dfc2942cc71b2def4cdcc766422d8ae23bbb2809de223de705b9ce71e9";
const VERIFY_TIMEOUT_SECS: u64 = 10;
const OFFLINE_GRACE_DAYS: i64 = 7;
/// How often the running gateway re-verifies the licence. The startup
/// check is hard-fail; this background check switches the kill-switch
/// off when the server says the key is no longer active.
const REVERIFY_INTERVAL_SECS: u64 = 6 * 60 * 60; // 6 hours
const HEARTBEAT_INTERVAL_SECS: u64 = 5 * 60; // 5 minutes
const USAGE_REPORT_INTERVAL_SECS: u64 = 60;

/// A verified, ready-to-use licence.
#[derive(Debug, Clone)]
pub struct License {
    pub customer_id: String,
    pub tier: PlanTier,
    pub expires_at: Option<DateTime<Utc>>,
    pub from_cache: bool,
    pub max_nodes: Option<u32>,
    pub active_nodes: u32,
    pub allow_overage: bool,
    pub monthly_call_limit: Option<u64>,
}

#[derive(Debug, thiserror::Error)]
pub enum LicenseError {
    #[error("API key missing — set LYNXX_API_KEY or [license].api_key in config.toml")]
    Missing,
    #[error("API key format invalid (expected `lnx_live_…` or `lnx_test_…`)")]
    BadFormat,
    #[error("licence server rejected the key: {0}")]
    Rejected(String),
    #[error("licence server unreachable and no cached licence available: {0}")]
    Offline(String),
    #[error("cached licence expired (no successful verify in {0} days)")]
    StaleCache(i64),
    #[error("licence response malformed: {0}")]
    Malformed(String),
}

#[derive(Debug, Serialize, Deserialize)]
struct VerifyResponse {
    /// Server-side identifier — the licence service emits `customer_urn`
    /// (the cross-cutting URN convention). The `alias` keeps older
    /// licence-server deployments + offline caches written by older
    /// binaries usable.
    #[serde(alias = "customer_id")]
    customer_urn: String,
    tier: String,
    #[serde(default)]
    expires_at: Option<i64>,
    #[serde(default)]
    scopes: Vec<String>,
    #[serde(default)]
    max_nodes: Option<u32>,
    #[serde(default)]
    active_nodes: u32,
    #[serde(default)]
    monthly_call_limit: Option<u64>,
    #[serde(default)]
    allow_overage: bool,
}

#[derive(Debug, Serialize, Deserialize)]
struct CacheEntry {
    api_key_hash: String,
    customer_id: String,
    tier: String,
    expires_at: Option<i64>,
    verified_at: i64,
    #[serde(default)]
    max_nodes: Option<u32>,
    #[serde(default)]
    active_nodes: Option<u32>,
}

impl License {
    /// Load and verify the licence. Branches on configuration:
    ///   * **keyx mode** — when `[keyx]` is fully configured and
    ///     `[license].payload` + `[license].signature` are present, the
    ///     signed bundle is verified against keyx
    ///     `POST /v1/keys/{urn}/verify-license`.
    ///   * **legacy mode** — otherwise an opaque `lnx_live_…` API key is
    ///     sent to the billing server.
    ///
    /// Both modes hard-fail on rejection and fall back to the offline
    /// cache (`~/.lynxx/license.cache.json`) for up to
    /// `OFFLINE_GRACE_DAYS` on network failure.
    pub async fn load(
        license_cfg: &LicenseSettings,
        keyx_cfg: &KeyxSettings,
    ) -> Result<Self, LicenseError> {
        if keyx_cfg.is_configured() && !license_cfg.payload.trim().is_empty() {
            return Self::load_via_keyx(license_cfg, keyx_cfg).await;
        }
        Self::load_via_legacy(license_cfg).await
    }

    async fn load_via_legacy(license_cfg: &LicenseSettings) -> Result<Self, LicenseError> {
        let api_key = match env::var("LYNXX_API_KEY").ok() {
            Some(k) if !k.trim().is_empty() => k,
            _ => {
                let k = license_cfg.api_key.trim();
                if k.is_empty() {
                    return Err(LicenseError::Missing);
                }
                k.to_string()
            }
        };
        let api_key = api_key.trim().to_string();

        if let Some(lic) = try_dev_key(&api_key) {
            return Ok(lic);
        }

        if !is_valid_format(&api_key) {
            return Err(LicenseError::BadFormat);
        }

        let vurl = verify_url();

        match verify_online(&api_key, &vurl).await {
            Ok(resp) => {
                let lic = License::from_verify_response(&resp, false);
                let _ = write_cache(&api_key, &resp);
                Ok(lic)
            }
            Err(LicenseError::Rejected(_)) => Err(LicenseError::Rejected(
                "key not active (cancelled or invalid)".into(),
            )),
            Err(net_err) => match read_cache(&api_key)? {
                Some(entry) => License::from_cache_entry(entry),
                None => Err(net_err),
            },
        }
    }

    async fn load_via_keyx(
        license_cfg: &LicenseSettings,
        keyx_cfg: &KeyxSettings,
    ) -> Result<Self, LicenseError> {
        use base64::engine::general_purpose::STANDARD as B64;
        use base64::Engine;

        let payload = B64
            .decode(license_cfg.payload.trim())
            .map_err(|e| LicenseError::Malformed(format!("license.payload not base64: {e}")))?;
        let parsed: VerifyResponse = serde_json::from_slice(&payload)
            .map_err(|e| LicenseError::Malformed(format!("license payload JSON: {e}")))?;
        let signature = license_cfg.signature.trim().to_string();

        let client = build_keyx_client(keyx_cfg)?;

        // The signature serves as the cache key in keyx mode — it's
        // unique per issued licence and shouldn't change across
        // restarts unless the operator pastes a new bundle.
        let cache_id = signature.as_str();

        match client.verify_license(&payload, &signature).await {
            Ok(true) => {
                let lic = License::from_verify_response(&parsed, false);
                let _ = write_cache(cache_id, &parsed);
                Ok(lic)
            }
            Ok(false) => Err(LicenseError::Rejected(
                "keyx rejected the licence signature (revoked or tampered)".into(),
            )),
            Err(KeyxError::Transport(e)) | Err(KeyxError::Http { body: e, .. }) => {
                match read_cache(cache_id)? {
                    Some(entry) => License::from_cache_entry(entry),
                    None => Err(LicenseError::Offline(e)),
                }
            }
            Err(KeyxError::Malformed(e)) => Err(LicenseError::Malformed(e)),
            Err(KeyxError::NotConfigured) => Err(LicenseError::Missing),
        }
    }

    fn from_verify_response(resp: &VerifyResponse, from_cache: bool) -> Self {
        License {
            customer_id: resp.customer_urn.clone(),
            tier: PlanTier::from_str(&resp.tier),
            expires_at: resp
                .expires_at
                .and_then(|ts| DateTime::<Utc>::from_timestamp(ts, 0)),
            from_cache,
            max_nodes: resp.max_nodes,
            active_nodes: resp.active_nodes,
            allow_overage: resp.allow_overage,
            monthly_call_limit: resp.monthly_call_limit,
        }
    }

    fn from_cache_entry(entry: CacheEntry) -> Result<Self, LicenseError> {
        let age_days = (Utc::now().timestamp() - entry.verified_at).max(0) / 86_400;
        if age_days > OFFLINE_GRACE_DAYS {
            return Err(LicenseError::StaleCache(age_days));
        }
        let tier = PlanTier::from_str(&entry.tier);
        Ok(License {
            customer_id: entry.customer_id,
            tier,
            expires_at: entry
                .expires_at
                .and_then(|ts| DateTime::<Utc>::from_timestamp(ts, 0)),
            from_cache: true,
            max_nodes: entry.max_nodes,
            active_nodes: entry.active_nodes.unwrap_or(0),
            allow_overage: matches!(tier, PlanTier::Pro | PlanTier::Business),
            monthly_call_limit: tier.monthly_limit(),
        })
    }

    /// Days until the licence expires, clamped at zero. `None` for
    /// perpetual (Business/Enterprise without expiry).
    pub fn days_remaining(&self) -> Option<i64> {
        self.expires_at
            .map(|exp| (exp - Utc::now()).num_days().max(0))
    }
}

/// Live licence state shared across the gateway. Wraps an
/// `AtomicBool` kill-switch that the periodic re-verifier flips off
/// when the licence server reports the key is no longer active —
/// a customer cancellation kicks in within `REVERIFY_INTERVAL_SECS`
/// instead of waiting `OFFLINE_GRACE_DAYS` for the cache to expire.
#[derive(Clone)]
pub struct LicenseState {
    inner: std::sync::Arc<LicenseStateInner>,
}

struct LicenseStateInner {
    api_key: String,
    verify_url: String,
    /// Present when this state was built from keyx mode. The background
    /// re-verifier uses it to re-check the signed bundle on the keyx
    /// schedule; absent ⇒ legacy mode (refresher hits `verify_url`).
    keyx_refresh: Option<KeyxRefresh>,
    /// `true` while the licence is good. Flipped to `false` on
    /// confirmed rejection from the licence server. Network failures
    /// and parse errors do NOT flip it — the offline cache plus the
    /// 7-day grace already cover transient outages.
    active: std::sync::atomic::AtomicBool,
    /// Latest known tier. Loaded at startup; refreshed on every
    /// successful re-verify so a tier upgrade / downgrade applies
    /// without a restart.
    tier: std::sync::RwLock<PlanTier>,
    node_counts: std::sync::RwLock<(u32, Option<u32>)>,
}

#[derive(Clone)]
struct KeyxRefresh {
    client: KeyxClient,
    payload: Vec<u8>,
    signature: String,
}

impl LicenseState {
    /// Build a legacy-mode state (background refresher hits the billing
    /// server with the API key + bearer auth).
    pub fn new(
        api_key: impl Into<String>,
        verify_url: impl Into<String>,
        tier: PlanTier,
        active_nodes: u32,
        max_nodes: Option<u32>,
    ) -> Self {
        Self {
            inner: std::sync::Arc::new(LicenseStateInner {
                api_key: api_key.into(),
                verify_url: verify_url.into(),
                keyx_refresh: None,
                active: std::sync::atomic::AtomicBool::new(true),
                tier: std::sync::RwLock::new(tier),
                node_counts: std::sync::RwLock::new((active_nodes, max_nodes)),
            }),
        }
    }

    /// Build a keyx-mode state. The background refresher re-checks the
    /// signed `(payload, signature)` bundle against keyx — there's no
    /// API key and no billing-server URL.
    pub fn new_keyx(
        keyx_cfg: &KeyxSettings,
        payload: Vec<u8>,
        signature: String,
        tier: PlanTier,
        active_nodes: u32,
        max_nodes: Option<u32>,
    ) -> Result<Self, LicenseError> {
        let client = build_keyx_client(keyx_cfg)?;
        Ok(Self {
            inner: std::sync::Arc::new(LicenseStateInner {
                api_key: String::new(),
                verify_url: String::new(),
                keyx_refresh: Some(KeyxRefresh {
                    client,
                    payload,
                    signature,
                }),
                active: std::sync::atomic::AtomicBool::new(true),
                tier: std::sync::RwLock::new(tier),
                node_counts: std::sync::RwLock::new((active_nodes, max_nodes)),
            }),
        })
    }

    /// Cheap check used in the proxy hot path. Returns `false` after
    /// the re-verifier sees a rejection.
    pub fn is_active(&self) -> bool {
        self.inner.active.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// `"keyx"` when this state was built from a signed bundle that's
    /// re-verified against keyx; `"legacy"` for the original
    /// billing-server API-key flow. Surfaced on the admin /status
    /// endpoint so operators can verify their config took.
    pub fn mode(&self) -> &'static str {
        if self.inner.keyx_refresh.is_some() {
            "keyx"
        } else {
            "legacy"
        }
    }

    /// Latest tier from the most recent successful re-verify.
    pub fn tier(&self) -> PlanTier {
        *self.inner.tier.read().expect("tier lock poisoned")
    }

    pub fn active_nodes(&self) -> u32 {
        self.inner.node_counts.read().expect("node lock poisoned").0
    }

    pub fn max_nodes(&self) -> Option<u32> {
        self.inner.node_counts.read().expect("node lock poisoned").1
    }

    fn update_node_counts(&self, active: u32, max: Option<u32>) {
        let mut nc = self.inner.node_counts.write().expect("node lock poisoned");
        *nc = (active, max);
    }

    fn flip_inactive(&self, reason: &str) {
        self.inner
            .active
            .store(false, std::sync::atomic::Ordering::Relaxed);
        tracing::error!(
            reason,
            "licence revoked — gateway is in kill-switch mode; all requests will return 402"
        );
    }

    fn update_tier(&self, new_tier: PlanTier) {
        let mut t = self.inner.tier.write().expect("tier lock poisoned");
        if *t != new_tier {
            tracing::info!(old = %t.as_str(), new = %new_tier.as_str(), "licence tier updated");
            *t = new_tier;
        }
    }
}

/// Spawn the background re-verifier. Branches on the mode the state
/// was built with:
///   * **legacy** — hits the billing server every `REVERIFY_INTERVAL_SECS`
///     with the API key. Tier / quota updates apply live. Rejection
///     flips the kill-switch.
///   * **keyx** — re-verifies the signed bundle against keyx on the
///     same interval. Tier / quota are immutable for a given signature
///     (they're in the signed payload), so this only flips the
///     kill-switch on a confirmed `valid = false` response.
///
/// Network failures leave the switch alone — the 7-day offline cache
/// covers transient outages.
pub fn spawn_license_refresher(
    state: LicenseState,
    quota: Option<std::sync::Arc<crate::quota::QuotaTracker>>,
) {
    if state.inner.keyx_refresh.is_some() {
        spawn_keyx_refresher(state);
        return;
    }
    if state.inner.api_key.starts_with("lnx_dev_") {
        return;
    }
    spawn_legacy_refresher(state, quota);
}

fn spawn_legacy_refresher(
    state: LicenseState,
    quota: Option<std::sync::Arc<crate::quota::QuotaTracker>>,
) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(StdDuration::from_secs(REVERIFY_INTERVAL_SECS));
        interval.tick().await;
        loop {
            interval.tick().await;
            match verify_online(&state.inner.api_key, &state.inner.verify_url).await {
                Ok(resp) => {
                    state.update_tier(PlanTier::from_str(&resp.tier));
                    state.update_node_counts(resp.active_nodes, resp.max_nodes);
                    if let Some(ref q) = quota {
                        q.set_limit(resp.monthly_call_limit);
                        q.set_allow_overage(resp.allow_overage);
                    }
                    let _ = write_cache(&state.inner.api_key, &resp);
                }
                Err(LicenseError::Rejected(msg)) => state.flip_inactive(&msg),
                Err(other) => {
                    tracing::warn!(error = %other, "licence re-verify failed; keeping current state");
                }
            }
        }
    });
}

fn spawn_keyx_refresher(state: LicenseState) {
    let keyx = state
        .inner
        .keyx_refresh
        .clone()
        .expect("keyx_refresh presence checked by caller");
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(StdDuration::from_secs(REVERIFY_INTERVAL_SECS));
        interval.tick().await;
        loop {
            interval.tick().await;
            match keyx
                .client
                .verify_license(&keyx.payload, &keyx.signature)
                .await
            {
                Ok(true) => tracing::debug!("keyx licence re-verify ok"),
                Ok(false) => {
                    state.flip_inactive("keyx says licence is no longer valid (revoked)")
                }
                Err(e) => {
                    tracing::warn!(error = %e, "keyx re-verify failed; keeping current state")
                }
            }
        }
    });
}

fn build_keyx_client(keyx_cfg: &KeyxSettings) -> Result<KeyxClient, LicenseError> {
    let cfg = KeyxConfig {
        base_url: keyx_cfg.base_url.clone(),
        service_token: keyx_cfg.service_token.clone(),
        license_urn: keyx_cfg.license_urn.clone(),
    };
    KeyxClient::new(cfg).map_err(|e| LicenseError::Rejected(format!("keyx client: {e}")))
}

// ---------------------------------------------------------------------------
// Node registration + heartbeat + usage reporting
// ---------------------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum NodeError {
    #[error("node limit reached ({active}/{max}); upgrade to add more nodes")]
    LimitReached { active: u32, max: u32 },
    #[error("node registration failed: {0}")]
    RegisterFailed(String),
}

#[derive(Debug, Serialize)]
struct RegisterReq {
    instance_id: String,
    hostname: String,
    version: Option<String>,
    ip_address: Option<String>,
    os: Option<String>,
    arch: Option<String>,
    data_dir: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RegisterResp {
    #[allow(dead_code)]
    node_id: i64,
    active_nodes: i64,
    max_nodes: Option<i32>,
}

/// Register this gateway node with the billing API. Returns
/// `NodeError::LimitReached` when the customer has reached their node
/// quota; the caller can choose to log and continue (the licence itself
/// is still valid). Skips silently for `lnx_dev_*` keys.
pub async fn register_node(api_key: &str, lynxx_base_url: &str) -> Result<(), NodeError> {
    if api_key.starts_with("lnx_dev_") {
        return Ok(());
    }
    let instance_id = compute_instance_id(api_key);
    let hostname = gethostname();
    let data_dir = dirs::home_dir()
        .map(|d| d.join(".lynxx").to_string_lossy().to_string());

    let url = format!(
        "{}/v1/nodes/register",
        lynxx_base_url.trim_end_matches('/'),
    );
    let body = RegisterReq {
        instance_id,
        hostname,
        version: Some(env!("CARGO_PKG_VERSION").to_string()),
        ip_address: None,
        os: Some(std::env::consts::OS.to_string()),
        arch: Some(std::env::consts::ARCH.to_string()),
        data_dir,
    };

    let client = reqwest::Client::builder()
        .timeout(StdDuration::from_secs(VERIFY_TIMEOUT_SECS))
        .build()
        .map_err(|e| NodeError::RegisterFailed(e.to_string()))?;

    let resp = client
        .post(&url)
        .bearer_auth(api_key)
        .json(&body)
        .send()
        .await
        .map_err(|e| NodeError::RegisterFailed(e.to_string()))?;

    let status = resp.status();
    if status == reqwest::StatusCode::FORBIDDEN {
        let text = resp.text().await.unwrap_or_default();
        if text.contains("node limit") {
            return Err(NodeError::LimitReached { active: 0, max: 0 });
        }
        return Err(NodeError::RegisterFailed(text));
    }
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(NodeError::RegisterFailed(format!("{status}: {text}")));
    }

    let reg: RegisterResp = resp
        .json()
        .await
        .map_err(|e| NodeError::RegisterFailed(e.to_string()))?;

    tracing::info!(
        active_nodes = reg.active_nodes,
        max_nodes = ?reg.max_nodes,
        "node registered with billing service"
    );
    Ok(())
}

/// Spawn the background heartbeat loop. Sends a heartbeat to the
/// billing API every `HEARTBEAT_INTERVAL_SECS` so the system knows
/// which nodes are alive. Skips silently for `lnx_dev_*` keys.
pub fn spawn_node_heartbeat(api_key: String, lynxx_base_url: String) {
    if api_key.starts_with("lnx_dev_") {
        return;
    }
    let instance_id = compute_instance_id(&api_key);
    let heartbeat_url = format!("{}/v1/nodes/heartbeat", lynxx_base_url.trim_end_matches('/'));

    tokio::spawn(async move {
        let mut interval = tokio::time::interval(StdDuration::from_secs(HEARTBEAT_INTERVAL_SECS));
        interval.tick().await; // skip immediate tick
        let client = reqwest::Client::builder()
            .timeout(StdDuration::from_secs(VERIFY_TIMEOUT_SECS))
            .build()
            .unwrap();
        loop {
            interval.tick().await;
            let body = serde_json::json!({ "instance_id": instance_id });
            match client
                .post(&heartbeat_url)
                .bearer_auth(&api_key)
                .json(&body)
                .send()
                .await
            {
                Ok(resp) if resp.status().is_success() => {
                    tracing::debug!("node heartbeat sent");
                }
                Ok(resp) => {
                    tracing::warn!(status = %resp.status(), "node heartbeat rejected");
                }
                Err(e) => {
                    tracing::warn!(error = %e, "node heartbeat failed");
                }
            }
        }
    });
}

/// Deregister this node on clean shutdown. Best-effort — tolerates
/// network failure and only logs. Skips silently for `lnx_dev_*` keys.
pub async fn deregister_node(api_key: &str, lynxx_base_url: &str) {
    if api_key.starts_with("lnx_dev_") {
        return;
    }
    let instance_id = compute_instance_id(api_key);
    let url = format!(
        "{}/v1/nodes/deregister",
        lynxx_base_url.trim_end_matches('/'),
    );
    let body = serde_json::json!({ "instance_id": instance_id });
    let client = reqwest::Client::builder()
        .timeout(StdDuration::from_secs(5))
        .build();
    if let Ok(client) = client {
        match client
            .post(&url)
            .bearer_auth(api_key)
            .json(&body)
            .send()
            .await
        {
            Ok(resp) if resp.status().is_success() => {
                tracing::info!("node deregistered");
            }
            Ok(resp) => {
                tracing::warn!(status = %resp.status(), "node deregister rejected");
            }
            Err(e) => {
                tracing::warn!(error = %e, "node deregister failed");
            }
        }
    }
}

/// Spawn the background usage reporter. Pushes this node's call counts
/// to the billing API every `USAGE_REPORT_INTERVAL_SECS` so overage
/// billing can aggregate across all customer nodes. If the response
/// contains `flagged=true`, flips the licence kill-switch off. Skips
/// silently for `lnx_dev_*` keys.
pub fn spawn_usage_reporter(
    api_key: String,
    lynxx_base_url: String,
    quota: std::sync::Arc<crate::quota::QuotaTracker>,
    license_state: LicenseState,
) {
    if api_key.starts_with("lnx_dev_") {
        return;
    }
    let instance_id = compute_instance_id(&api_key);
    let report_url = format!("{}/v1/usage/report", lynxx_base_url.trim_end_matches('/'));

    tokio::spawn(async move {
        let mut interval =
            tokio::time::interval(StdDuration::from_secs(USAGE_REPORT_INTERVAL_SECS));
        interval.tick().await;
        let client = reqwest::Client::builder()
            .timeout(StdDuration::from_secs(VERIFY_TIMEOUT_SECS))
            .build()
            .unwrap();
        loop {
            interval.tick().await;
            let snap = quota.snapshot();
            let body = serde_json::json!({
                "instance_id": instance_id,
                "period": snap.period,
                "calls_total": snap.calls_total,
            });
            match client
                .post(&report_url)
                .bearer_auth(&api_key)
                .json(&body)
                .send()
                .await
            {
                Ok(resp) if resp.status().is_success() => {
                    if let Ok(json) = resp.json::<serde_json::Value>().await {
                        if json.get("flagged").and_then(|v| v.as_bool()).unwrap_or(false) {
                            license_state.flip_inactive(
                                "tamper detected — account suspended by billing server",
                            );
                        }
                    }
                    tracing::debug!(calls = snap.calls_total, "usage report sent");
                }
                Ok(resp) => {
                    tracing::warn!(status = %resp.status(), "usage report rejected");
                }
                Err(e) => {
                    tracing::warn!(error = %e, "usage report failed");
                }
            }
        }
    });
}

// ---------------------------------------------------------------------------
// Instance identity (used by admin status endpoint)
// ---------------------------------------------------------------------------

/// Deterministic instance ID for this gateway installation. Hashes
/// hostname + data directory + API key so the same physical install
/// always gets the same ID.
pub fn compute_instance_id(api_key: &str) -> String {
    use sha2::{Digest, Sha256};
    let hostname = gethostname();
    let data_dir = dirs::home_dir()
        .map(|d| d.join(".lynxx").to_string_lossy().to_string())
        .unwrap_or_default();
    let mut h = Sha256::new();
    h.update(hostname.as_bytes());
    h.update(b":");
    h.update(data_dir.as_bytes());
    h.update(b":");
    h.update(api_key.as_bytes());
    format!("node_{:x}", h.finalize())[..24].to_string()
}

fn is_valid_format(key: &str) -> bool {
    let prefixes = ["lnx_live_", "lnx_test_"];
    prefixes.iter().any(|p| key.starts_with(p)) && key.len() >= 16
}

/// Debug-build escape hatch — `lnx_dev_<tier>` bypasses the server call.
/// Release builds always return `None`, so shipped binaries can't be
/// bypassed by guessing a magic key.
#[cfg(debug_assertions)]
fn try_dev_key(api_key: &str) -> Option<License> {
    let tier_str = api_key.strip_prefix("lnx_dev_")?;
    let tier = match tier_str.to_lowercase().as_str() {
        "free" | "pro" | "business" | "enterprise" => PlanTier::from_str(tier_str),
        _ => return None,
    };
    Some(License {
        customer_id: format!("dev_{}", tier.as_str()),
        tier,
        expires_at: None,
        from_cache: false,
        max_nodes: None,
        active_nodes: 0,
        allow_overage: matches!(tier, PlanTier::Pro | PlanTier::Business),
        monthly_call_limit: tier.monthly_limit(),
    })
}

#[cfg(not(debug_assertions))]
fn try_dev_key(_api_key: &str) -> Option<License> {
    None
}

async fn verify_online(api_key: &str, url: &str) -> Result<VerifyResponse, LicenseError> {
    let client = reqwest::Client::builder()
        .timeout(StdDuration::from_secs(VERIFY_TIMEOUT_SECS))
        .build()
        .map_err(|e| LicenseError::Offline(e.to_string()))?;

    let resp = client
        .get(url)
        .bearer_auth(api_key)
        .send()
        .await
        .map_err(|e| LicenseError::Offline(e.to_string()))?;

    let status = resp.status();
    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        let body = resp.text().await.unwrap_or_default();
        return Err(LicenseError::Rejected(format!("{status}: {body}")));
    }
    if !status.is_success() {
        return Err(LicenseError::Offline(format!("server returned {status}")));
    }

    // Extract the Ed25519 signature header before consuming the body.
    let sig_hex = resp
        .headers()
        .get("x-lynxx-sig")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    let body_bytes = resp
        .bytes()
        .await
        .map_err(|e| LicenseError::Malformed(format!("read body: {e}")))?;

    // Verify the signature if the server sent one. Release builds
    // REQUIRE a valid signature — missing or invalid = rejected.
    // Debug builds warn but continue (for local dev without the key).
    verify_response_signature(&body_bytes, sig_hex.as_deref())?;

    serde_json::from_slice::<VerifyResponse>(&body_bytes)
        .map_err(|e| LicenseError::Malformed(e.to_string()))
}

/// Verify the Ed25519 signature on the response body.
fn verify_response_signature(
    body: &[u8],
    sig_hex: Option<&str>,
) -> Result<(), LicenseError> {
    use ed25519_dalek::{Signature, Verifier, VerifyingKey};

    let pub_bytes = hex::decode(VERIFY_PUBLIC_KEY_HEX)
        .map_err(|_| LicenseError::Malformed("compiled public key is invalid hex".into()))?;
    let pub_key = VerifyingKey::from_bytes(
        pub_bytes
            .as_slice()
            .try_into()
            .map_err(|_| LicenseError::Malformed("compiled public key wrong length".into()))?,
    )
    .map_err(|_| LicenseError::Malformed("compiled public key invalid".into()))?;

    match sig_hex {
        Some(hex_str) => {
            let sig_bytes = hex::decode(hex_str).map_err(|_| {
                LicenseError::Rejected(
                    "verify response signature is invalid hex — possible MITM".into(),
                )
            })?;
            let signature = Signature::from_bytes(sig_bytes.as_slice().try_into().map_err(
                |_| {
                    LicenseError::Rejected(
                        "verify response signature wrong length — possible MITM".into(),
                    )
                },
            )?);
            pub_key.verify(body, &signature).map_err(|_| {
                LicenseError::Rejected(
                    "verify response signature INVALID — response tampered (possible MITM)".into(),
                )
            })?;
            tracing::debug!("verify response signature valid");
            Ok(())
        }
        None => {
            // No signature header. In release builds, this is fatal —
            // the billing server should always sign. In debug builds,
            // tolerate it for local dev without the signing key.
            if cfg!(debug_assertions) {
                tracing::warn!("verify response has no signature (debug build — tolerating)");
                Ok(())
            } else {
                Err(LicenseError::Rejected(
                    "verify response missing signature — possible MITM or downgrade attack".into(),
                ))
            }
        }
    }
}

fn cache_path() -> Option<PathBuf> {
    let dir = if let Ok(p) = env::var("LYNXX_CACHE_DIR") {
        PathBuf::from(p)
    } else {
        dirs::home_dir()?.join(".lynxx")
    };
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir.join("license.cache.json"))
}

fn key_fingerprint(api_key: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(api_key.as_bytes());
    format!("{:x}", h.finalize())
}

fn write_cache(api_key: &str, resp: &VerifyResponse) -> Result<(), std::io::Error> {
    let Some(path) = cache_path() else {
        return Ok(());
    };
    let entry = CacheEntry {
        api_key_hash: key_fingerprint(api_key),
        customer_id: resp.customer_urn.clone(),
        tier: resp.tier.clone(),
        expires_at: resp.expires_at,
        verified_at: Utc::now().timestamp(),
        max_nodes: resp.max_nodes,
        active_nodes: Some(resp.active_nodes),
    };
    let body = serde_json::to_vec_pretty(&entry).unwrap();
    std::fs::write(path, body)
}

fn read_cache(api_key: &str) -> Result<Option<CacheEntry>, LicenseError> {
    let Some(path) = cache_path() else {
        return Ok(None);
    };
    if !path.exists() {
        return Ok(None);
    }
    let body =
        std::fs::read(&path).map_err(|e| LicenseError::Offline(format!("cache read: {e}")))?;
    let entry: CacheEntry = serde_json::from_slice(&body)
        .map_err(|e| LicenseError::Malformed(format!("cache parse: {e}")))?;
    if entry.api_key_hash != key_fingerprint(api_key) {
        return Ok(None);
    }
    Ok(Some(entry))
}

fn gethostname() -> String {
    std::process::Command::new("hostname")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "unknown".to_string())
}

// Silence unused import warning when compiled in release mode (Duration
// is only referenced by some helpers via chrono internals).
#[allow(dead_code)]
fn _ensure_duration_used() -> Duration {
    Duration::days(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dev_key_pro() {
        let lic = try_dev_key("lnx_dev_pro").unwrap();
        assert_eq!(lic.tier, PlanTier::Pro);
        assert_eq!(lic.customer_id, "dev_pro");
    }

    #[test]
    fn dev_key_unknown_tier_rejected() {
        assert!(try_dev_key("lnx_dev_godmode").is_none());
    }

    #[test]
    fn rejects_garbage_format() {
        assert!(!is_valid_format("not-a-key"));
        assert!(!is_valid_format("lnx_live_short"));
        assert!(is_valid_format("lnx_live_abc123def456"));
        assert!(is_valid_format("lnx_test_abc123def456"));
    }

    #[test]
    fn key_fingerprint_stable_and_distinct() {
        let a = key_fingerprint("lnx_live_aaa");
        let b = key_fingerprint("lnx_live_aaa");
        let c = key_fingerprint("lnx_live_bbb");
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_eq!(a.len(), 64);
    }

    #[test]
    fn license_state_legacy_constructor_reports_legacy_mode() {
        let s = LicenseState::new("k", "u", PlanTier::Pro, 0, None);
        assert_eq!(s.mode(), "legacy");
    }

    #[test]
    fn license_state_keyx_constructor_reports_keyx_mode() {
        let cfg = crate::config::KeyxSettings {
            base_url: "http://x".into(),
            service_token: "t".into(),
            license_urn: "00000000-0000-0000-0000-000000000001".into(),
            ..Default::default()
        };
        let s = LicenseState::new_keyx(&cfg, vec![1, 2], "sig".into(), PlanTier::Pro, 0, None)
            .unwrap();
        assert_eq!(s.mode(), "keyx");
    }
}

/// Integration tests for the keyx code path through `License::load`.
/// These mutate process env (HOME → tempdir, LYNXX_CACHE_DIR, and
/// clear LYNXX_API_KEY). Serialised via a single global mutex so they
/// don't trip each other or the legacy tests above.
#[cfg(test)]
mod keyx_load_tests {
    use super::*;
    use crate::config::{KeyxSettings, LicenseSettings};
    use base64::engine::general_purpose::STANDARD as B64;
    use base64::Engine;
    use std::sync::Mutex;
    use wiremock::matchers::{header, method, path as wm_path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    const URN: &str = "00000000-0000-0000-0000-000000000099";

    fn sample_payload_b64() -> String {
        let json = serde_json::json!({
            "customer_urn": "cus_abc",
            "tier": "pro",
            "expires_at": 1893456000_i64,
            "max_nodes": 5,
            "active_nodes": 1,
            "allow_overage": true,
            "monthly_call_limit": 1_000_000,
        });
        B64.encode(serde_json::to_vec(&json).unwrap())
    }

    fn keyx_cfg(uri: String) -> KeyxSettings {
        KeyxSettings {
            base_url: uri,
            service_token: "svc".into(),
            license_urn: URN.into(),
            ..Default::default()
        }
    }

    fn lic_cfg(payload: String, signature: String) -> LicenseSettings {
        LicenseSettings {
            api_key: String::new(),
            payload,
            signature,
        }
    }

    fn envelope(data: serde_json::Value) -> serde_json::Value {
        serde_json::json!({
            "transactionUrn": "urn:keyx:txn:test",
            "status": "SUCCESS",
            "responseMessage": "OK",
            "responseKey": "SUCCESS",
            "errors": [],
            "timestamp": "2026-05-16T00:00:00+00:00",
            "metadata": {},
            "data": data,
        })
    }

    struct EnvGuard {
        _tmp: tempfile::TempDir,
        _lock: std::sync::MutexGuard<'static, ()>,
    }

    fn isolated_env() -> EnvGuard {
        let lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        env::set_var("LYNXX_CACHE_DIR", tmp.path());
        env::remove_var("LYNXX_API_KEY");
        EnvGuard {
            _tmp: tmp,
            _lock: lock,
        }
    }

    #[tokio::test]
    async fn keyx_mode_happy_path_returns_parsed_license() {
        let _e = isolated_env();
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(wm_path(format!("/v1/keys/{}/verify-license", URN)))
            .and(header("authorization", "Bearer svc"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(envelope(serde_json::json!({ "valid": true }))),
            )
            .mount(&server)
            .await;

        let lic = License::load(
            &lic_cfg(sample_payload_b64(), "sig".into()),
            &keyx_cfg(server.uri()),
        )
        .await
        .unwrap();

        assert_eq!(lic.customer_id, "cus_abc");
        assert_eq!(lic.tier, PlanTier::Pro);
        assert_eq!(lic.max_nodes, Some(5));
        assert_eq!(lic.monthly_call_limit, Some(1_000_000));
        assert!(lic.allow_overage);
        assert!(!lic.from_cache);
    }

    #[tokio::test]
    async fn keyx_mode_rejects_when_keyx_says_invalid() {
        let _e = isolated_env();
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(envelope(serde_json::json!({ "valid": false }))),
            )
            .mount(&server)
            .await;

        let err = License::load(
            &lic_cfg(sample_payload_b64(), "tampered".into()),
            &keyx_cfg(server.uri()),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, LicenseError::Rejected(_)));
    }

    #[tokio::test]
    async fn keyx_mode_offline_without_cache_returns_offline_error() {
        let _e = isolated_env();
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(503).set_body_string("down"))
            .mount(&server)
            .await;

        let err = License::load(
            &lic_cfg(sample_payload_b64(), "sig-fresh".into()),
            &keyx_cfg(server.uri()),
        )
        .await
        .unwrap_err();
        assert!(
            matches!(err, LicenseError::Offline(_)),
            "expected Offline, got {err:?}"
        );
    }

    #[tokio::test]
    async fn keyx_mode_offline_falls_back_to_warm_cache() {
        let _e = isolated_env();
        let server = MockServer::start().await;
        let ok_mock = Mock::given(method("POST"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(envelope(serde_json::json!({ "valid": true }))),
            )
            .up_to_n_times(1)
            .mount_as_scoped(&server)
            .await;

        let cfg_l = lic_cfg(sample_payload_b64(), "sig-warm".into());
        let cfg_k = keyx_cfg(server.uri());

        // Seed cache via happy path.
        let _seed = License::load(&cfg_l, &cfg_k).await.unwrap();
        drop(ok_mock);

        // Subsequent failure: cache rescue.
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(503).set_body_string("down"))
            .mount(&server)
            .await;
        let lic = License::load(&cfg_l, &cfg_k).await.unwrap();
        assert!(lic.from_cache);
        assert_eq!(lic.customer_id, "cus_abc");
    }

    #[tokio::test]
    async fn legacy_mode_runs_when_keyx_unconfigured() {
        let _e = isolated_env();
        // No keyx config + no api_key → Missing error (legacy path picked).
        let err = License::load(&LicenseSettings::default(), &KeyxSettings::default())
            .await
            .unwrap_err();
        assert!(matches!(err, LicenseError::Missing));
    }
}
