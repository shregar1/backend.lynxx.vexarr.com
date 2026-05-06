//! Monthly API call quota enforcement.
//!
//! Each Lynxx licence tier carries a monthly call limit:
//!
//!   free       →  1 000 000 calls / month
//!   pro        → 10 000 000 calls / month
//!   business   →  unlimited
//!   enterprise →  unlimited
//!
//! The tracker keeps an in-memory `AtomicU64` counter that is loaded from the
//! DB on startup and flushed back every `FLUSH_INTERVAL_SECS` seconds. This
//! means quota checks cost a single atomic increment with no DB round-trip per
//! request, while usage survives a restart to within one flush window.
//!
//! Month rollover is detected on each flush: if the current calendar month
//! differs from the stored period the counters reset to zero.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use serde::Serialize;
use tokio::time::interval;
use tracing::{info, warn};

use crate::db::Repository;
use crate::error::GatewayError;

/// Seconds between DB flushes.
const FLUSH_INTERVAL_SECS: u64 = 30;

// ---------------------------------------------------------------------------
// Plan tier
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PlanTier {
    Free,
    Pro,
    Business,
    Enterprise,
}

impl PlanTier {
    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "pro" => PlanTier::Pro,
            "business" => PlanTier::Business,
            "enterprise" => PlanTier::Enterprise,
            _ => PlanTier::Free,
        }
    }

    /// Monthly call limit, or `None` for unlimited plans.
    pub fn monthly_limit(self) -> Option<u64> {
        match self {
            PlanTier::Free => Some(1_000_000),
            PlanTier::Pro => Some(10_000_000),
            PlanTier::Business => None,
            PlanTier::Enterprise => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            PlanTier::Free => "free",
            PlanTier::Pro => "pro",
            PlanTier::Business => "business",
            PlanTier::Enterprise => "enterprise",
        }
    }
}

// ---------------------------------------------------------------------------
// QuotaTracker
// ---------------------------------------------------------------------------

/// Thread-safe, lock-free quota tracker. Wire one instance into the gateway
/// at startup, share it as `Arc<QuotaTracker>` across the proxy handler and
/// the admin API.
pub struct QuotaTracker {
    plan: PlanTier,
    /// Current billing period ('YYYY-MM'). Updated atomically on rollover.
    period: std::sync::Mutex<String>,
    /// Calls allowed through this period.
    calls: AtomicU64,
    /// Calls rejected due to over-quota this period.
    rejected: AtomicU64,
    repo: Arc<dyn Repository>,
}

impl QuotaTracker {
    /// Load the current period's counters from the DB and return a ready
    /// tracker. Call this once during gateway startup.
    pub async fn load(
        plan: PlanTier,
        repo: Arc<dyn Repository>,
    ) -> Result<Arc<Self>, GatewayError> {
        let period = current_period();
        let (calls, rejected) = repo
            .get_quota_usage(&period)
            .await?
            .map(|(c, r)| (c, r))
            .unwrap_or((0, 0));

        let tracker = Arc::new(Self {
            plan,
            period: std::sync::Mutex::new(period.clone()),
            calls: AtomicU64::new(calls),
            rejected: AtomicU64::new(rejected),
            repo,
        });

        info!(
            plan = plan.as_str(),
            period = %period,
            calls,
            limit = ?plan.monthly_limit(),
            "quota tracker initialised"
        );

        Ok(tracker)
    }

    /// Check whether this request should be allowed.
    ///
    /// * If the plan is unlimited → always `true`.
    /// * If `calls < limit` → increment calls, return `true`.
    /// * Otherwise → increment rejected, return `false`.
    ///
    /// The check-then-increment is non-atomic: a brief burst of requests
    /// arriving simultaneously may allow a few extra calls past the cap.
    /// This is acceptable; the alternative (a mutex on every request) would
    /// destroy throughput.
    pub fn check_and_count(&self) -> bool {
        self.maybe_rollover();

        let Some(limit) = self.plan.monthly_limit() else {
            // Unlimited plan — count but never reject.
            self.calls.fetch_add(1, Ordering::Relaxed);
            return true;
        };

        // Optimistic increment: fetch the value BEFORE adding so we can
        // check against the limit without a separate read.
        let prev = self.calls.fetch_add(1, Ordering::Relaxed);
        if prev < limit {
            true
        } else {
            // Undo the increment and count as rejected.
            self.calls.fetch_sub(1, Ordering::Relaxed);
            self.rejected.fetch_add(1, Ordering::Relaxed);
            false
        }
    }

    /// Snapshot of current usage for the admin API / dashboard.
    pub fn snapshot(&self) -> QuotaSnapshot {
        let period = self.period.lock().expect("period mutex").clone();
        let calls = self.calls.load(Ordering::Relaxed);
        let rejected = self.rejected.load(Ordering::Relaxed);
        let limit = self.plan.monthly_limit();
        QuotaSnapshot {
            plan: self.plan,
            period,
            calls_total: calls,
            calls_rejected: rejected,
            calls_limit: limit,
            calls_remaining: limit.map(|l| l.saturating_sub(calls)),
        }
    }

    /// Persist counters to the DB. Called by the background flush task and
    /// on clean shutdown.
    pub async fn flush(&self) {
        let period = self.period.lock().expect("period mutex").clone();
        let calls = self.calls.load(Ordering::Relaxed);
        let rejected = self.rejected.load(Ordering::Relaxed);
        if let Err(e) = self.repo.upsert_quota_usage(&period, calls, rejected).await {
            warn!(error = %e, "quota flush failed");
        }
    }

    // -----------------------------------------------------------------------

    /// If the calendar month has changed, reset counters and update the period.
    fn maybe_rollover(&self) {
        let now = current_period();
        let mut period = self.period.lock().expect("period mutex");
        if *period != now {
            info!(old = %period, new = %now, "quota period rollover — resetting counters");
            *period = now;
            self.calls.store(0, Ordering::Relaxed);
            self.rejected.store(0, Ordering::Relaxed);
        }
    }
}

// ---------------------------------------------------------------------------
// Background flush task
// ---------------------------------------------------------------------------

/// Spawn a background task that flushes quota counters to the DB every
/// `FLUSH_INTERVAL_SECS` seconds.  The handle is fire-and-forget; dropping
/// it does not cancel the task (the `Arc<QuotaTracker>` keeps the tracker
/// alive).
pub fn spawn_quota_flusher(tracker: Arc<QuotaTracker>) {
    tokio::spawn(async move {
        let mut ticker = interval(Duration::from_secs(FLUSH_INTERVAL_SECS));
        ticker.tick().await; // skip the immediate first tick
        loop {
            ticker.tick().await;
            tracker.flush().await;
        }
    });
}

// ---------------------------------------------------------------------------
// Snapshot (used by admin API)
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct QuotaSnapshot {
    pub plan: PlanTier,
    pub period: String,
    pub calls_total: u64,
    pub calls_rejected: u64,
    pub calls_limit: Option<u64>,
    pub calls_remaining: Option<u64>,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn current_period() -> String {
    Utc::now().format("%Y-%m").to_string()
}
