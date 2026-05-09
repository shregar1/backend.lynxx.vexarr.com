//! License gating — simple API-key model.
//!
//! The user pastes a short key like `lnx_live_abc123` into config or
//! the `LYNXX_API_KEY` env var. On startup the gateway calls the licence
//! server to verify the key; on success it caches the result locally so
//! subsequent restarts work offline for up to 7 days.
//!
//! ## Verify request
//!
//! `GET ${verify_url}?key=<api_key>`  (default `verify_url`:
//! `https://api.lynxx.vexarr.com/v1/license/verify`)
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

use crate::quota::PlanTier;

const DEFAULT_VERIFY_URL: &str = "https://api.lynxx.vexarr.com/v1/license/verify";
const VERIFY_TIMEOUT_SECS: u64 = 10;
const OFFLINE_GRACE_DAYS: i64 = 7;
/// How often the running gateway re-verifies the licence. The startup
/// check is hard-fail; this background check switches the kill-switch
/// off when the server says the key is no longer active.
const REVERIFY_INTERVAL_SECS: u64 = 6 * 60 * 60; // 6 hours

/// A verified, ready-to-use licence.
#[derive(Debug, Clone)]
pub struct License {
    pub customer_id: String,
    pub tier: PlanTier,
    pub expires_at: Option<DateTime<Utc>>,
    /// True when the result came from the offline cache rather than a
    /// fresh server call. Logged at startup so operators know.
    pub from_cache: bool,
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
}

#[derive(Debug, Serialize, Deserialize)]
struct CacheEntry {
    api_key_hash: String,
    customer_id: String,
    tier: String,
    expires_at: Option<i64>,
    verified_at: i64,
}

impl License {
    /// Load an API key from env/config and verify it. Hard-fails the
    /// gateway when missing or rejected. Falls back to a local cache if
    /// the server is unreachable but a recent successful verify exists.
    pub async fn load(config_api_key: Option<&str>) -> Result<Self, LicenseError> {
        let api_key = match env::var("LYNXX_API_KEY").ok() {
            Some(k) if !k.trim().is_empty() => k,
            _ => match config_api_key {
                Some(k) if !k.trim().is_empty() => k.to_string(),
                _ => return Err(LicenseError::Missing),
            },
        };
        let api_key = api_key.trim().to_string();

        if let Some(lic) = try_dev_key(&api_key) {
            return Ok(lic);
        }

        if !is_valid_format(&api_key) {
            return Err(LicenseError::BadFormat);
        }

        let verify_url =
            env::var("LYNXX_LICENSE_VERIFY_URL").unwrap_or_else(|_| DEFAULT_VERIFY_URL.to_string());

        match verify_online(&api_key, &verify_url).await {
            Ok(resp) => {
                let lic = License {
                    customer_id: resp.customer_urn.clone(),
                    tier: PlanTier::from_str(&resp.tier),
                    expires_at: resp
                        .expires_at
                        .and_then(|ts| DateTime::<Utc>::from_timestamp(ts, 0)),
                    from_cache: false,
                };
                let _ = write_cache(&api_key, &resp);
                Ok(lic)
            }
            Err(LicenseError::Rejected(_)) => Err(LicenseError::Rejected(
                "key not active (cancelled or invalid)".into(),
            )),
            Err(net_err) => match read_cache(&api_key)? {
                Some(entry) => {
                    let age_days = (Utc::now().timestamp() - entry.verified_at).max(0) / 86_400;
                    if age_days > OFFLINE_GRACE_DAYS {
                        Err(LicenseError::StaleCache(age_days))
                    } else {
                        Ok(License {
                            customer_id: entry.customer_id,
                            tier: PlanTier::from_str(&entry.tier),
                            expires_at: entry
                                .expires_at
                                .and_then(|ts| DateTime::<Utc>::from_timestamp(ts, 0)),
                            from_cache: true,
                        })
                    }
                }
                None => Err(net_err),
            },
        }
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
    /// `true` while the licence is good. Flipped to `false` on
    /// confirmed rejection from the licence server. Network failures
    /// and parse errors do NOT flip it — the offline cache plus the
    /// 7-day grace already cover transient outages.
    active: std::sync::atomic::AtomicBool,
    /// Latest known tier. Loaded at startup; refreshed on every
    /// successful re-verify so a tier upgrade / downgrade applies
    /// without a restart.
    tier: std::sync::RwLock<PlanTier>,
}

impl LicenseState {
    pub fn new(api_key: impl Into<String>, verify_url: impl Into<String>, tier: PlanTier) -> Self {
        Self {
            inner: std::sync::Arc::new(LicenseStateInner {
                api_key: api_key.into(),
                verify_url: verify_url.into(),
                active: std::sync::atomic::AtomicBool::new(true),
                tier: std::sync::RwLock::new(tier),
            }),
        }
    }

    /// Cheap check used in the proxy hot path. Returns `false` after
    /// the re-verifier sees a rejection.
    pub fn is_active(&self) -> bool {
        self.inner.active.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Latest tier from the most recent successful re-verify.
    pub fn tier(&self) -> PlanTier {
        *self.inner.tier.read().expect("tier lock poisoned")
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

/// Spawn the background re-verifier. Hits the licence server every
/// `REVERIFY_INTERVAL_SECS` and:
///   - On success → updates the tier and refreshes the offline cache.
///   - On rejection → flips the kill-switch off; the proxy starts
///     returning 402 Payment Required for every request.
///   - On network failure → logs a warning, leaves the switch alone.
///     The 7-day cache still covers extended outages.
pub fn spawn_license_refresher(state: LicenseState) {
    // Skip the background loop entirely for dev keys (only present
    // in debug builds). They're not registered server-side and a
    // re-verify would always fail.
    if state.inner.api_key.starts_with("lnx_dev_") {
        return;
    }

    tokio::spawn(async move {
        let mut interval = tokio::time::interval(StdDuration::from_secs(REVERIFY_INTERVAL_SECS));
        // The first tick fires immediately; skip it because the
        // startup verify already ran a few seconds ago.
        interval.tick().await;
        loop {
            interval.tick().await;
            match verify_online(&state.inner.api_key, &state.inner.verify_url).await {
                Ok(resp) => {
                    state.update_tier(PlanTier::from_str(&resp.tier));
                    let _ = write_cache(&state.inner.api_key, &resp);
                }
                Err(LicenseError::Rejected(msg)) => {
                    state.flip_inactive(&msg);
                }
                Err(other) => {
                    tracing::warn!(error = %other, "licence re-verify failed; keeping current state");
                }
            }
        }
    });
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
        .query(&[("key", api_key)])
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

    resp.json::<VerifyResponse>()
        .await
        .map_err(|e| LicenseError::Malformed(e.to_string()))
}

fn cache_path() -> Option<PathBuf> {
    let dir = dirs::home_dir()?.join(".lynxx");
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
}
