//! Per-(field, key) token buckets for GraphQL field rate limits.
//!
//! Rate limits are simple fixed-window-style: each bucket gets `rps` tokens
//! and refills to capacity every second. We use `DashMap` for shard-free
//! contention; the field-rate-limit hot path is fast (microseconds per
//! lookup + atomic ops on the bucket).

use dashmap::DashMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug)]
struct Bucket {
    /// Tokens available *now*. Allowed to go negative briefly under
    /// contention; the next refill clamps back to capacity.
    tokens: AtomicI64,
    /// UNIX epoch second at which the bucket last refilled.
    last_refill: AtomicI64,
    capacity: i64,
}

impl Bucket {
    fn new(capacity: u32) -> Self {
        Self {
            tokens: AtomicI64::new(capacity as i64),
            last_refill: AtomicI64::new(now_secs()),
            capacity: capacity as i64,
        }
    }

    /// Try to consume one token. Returns true if allowed.
    fn try_consume(&self) -> bool {
        let now = now_secs();
        let last = self.last_refill.load(Ordering::Acquire);
        if now > last {
            // Refill — cheap CAS so concurrent callers don't all reset.
            if self
                .last_refill
                .compare_exchange(last, now, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                self.tokens.store(self.capacity, Ordering::Release);
            }
        }
        let prev = self.tokens.fetch_sub(1, Ordering::AcqRel);
        prev > 0
    }
}

#[derive(Debug, Default)]
pub struct FieldBuckets {
    /// Keyed by `(field_name, identity_key)`.
    inner: DashMap<(String, String), Bucket>,
}

impl FieldBuckets {
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns true if the request is allowed; false if rate-limited.
    pub fn try_consume(&self, field: &str, key: &str, rps: u32) -> bool {
        let composite = (field.to_string(), key.to_string());
        if let Some(b) = self.inner.get(&composite) {
            return b.try_consume();
        }
        // Insert race is fine — `entry().or_insert_with` ensures one writer
        // wins and the loser sees the freshly-inserted bucket.
        let entry = self
            .inner
            .entry(composite)
            .or_insert_with(|| Bucket::new(rps));
        entry.value().try_consume()
    }
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bucket_allows_capacity_then_denies() {
        let b = FieldBuckets::new();
        assert!(b.try_consume("deleteUser", "alice", 2));
        assert!(b.try_consume("deleteUser", "alice", 2));
        assert!(!b.try_consume("deleteUser", "alice", 2));
    }

    #[test]
    fn buckets_are_per_key() {
        let b = FieldBuckets::new();
        assert!(b.try_consume("deleteUser", "alice", 1));
        assert!(!b.try_consume("deleteUser", "alice", 1));
        // Different consumer gets its own bucket.
        assert!(b.try_consume("deleteUser", "bob", 1));
    }
}
