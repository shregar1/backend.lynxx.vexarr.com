//! Per-service load balancer with weighted round-robin, least-connections,
//! IP hash, cookie sticky-session, and consistent-hash (rendezvous / HRW)
//! strategies. Targets are runtime-tracked for in-flight count and health.

use crate::models::Target;
use dashmap::DashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use uuid::Uuid;

pub const STICKY_COOKIE_NAME: &str = "_lynxx_upstream";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LbStrategy {
    RoundRobin,
    LeastConn,
    IpHash,
    CookieSticky,
    ConsistentHash,
    Random,
    /// Power-of-2 random choices: pick 2 random healthy targets, forward to
    /// the one with fewer in-flight requests. Gives near-optimal spread
    /// without the coordination overhead of pure least-conn.
    RandomChoose,
    /// Primary / failover: always pick the first healthy, enabled target in
    /// target-list order. Traffic only moves to the next target when all
    /// preceding ones are unhealthy.
    First,
    /// Hash a configurable request header to pin sessions. The header name
    /// is stored on the Service model (`lb_hash_header`).
    HeaderHash,
    /// Hash a configurable query parameter to pin sessions. The param name
    /// is stored on the Service model (`lb_hash_query_param`).
    QueryHash,
    /// Hash the request URI path for consistent routing. Same path always
    /// goes to the same target.
    UriHash,
    /// Hash the X-Forwarded-For client IP (as opposed to IpHash which uses
    /// the direct remote/socket IP).
    ClientIpHash,
}

impl LbStrategy {
    pub fn from_str(s: &str) -> Self {
        match s {
            "least-conn" | "least_conn" | "leastconn" => LbStrategy::LeastConn,
            "ip-hash" | "ip_hash" | "iphash" => LbStrategy::IpHash,
            "cookie" | "cookie-sticky" | "sticky" => LbStrategy::CookieSticky,
            "consistent-hash" | "consistent_hash" | "consistenthash" => LbStrategy::ConsistentHash,
            "random" => LbStrategy::Random,
            "random-choose" | "random_choose" | "p2c" => LbStrategy::RandomChoose,
            "first" | "failover" | "primary" => LbStrategy::First,
            "header-hash" | "header_hash" | "headerhash" => LbStrategy::HeaderHash,
            "query-hash" | "query_hash" | "queryhash" => LbStrategy::QueryHash,
            "uri-hash" | "uri_hash" | "urihash" => LbStrategy::UriHash,
            "client-ip-hash" | "client_ip_hash" => LbStrategy::ClientIpHash,
            _ => LbStrategy::RoundRobin,
        }
    }
}

/// Request-scoped context for hash-based LB strategies.
pub struct RequestContext<'a> {
    pub client_ip: Option<&'a str>,
    pub uri_path: Option<&'a str>,
    pub sticky_cookie: Option<&'a str>,
    /// Value of the header named by `Service.lb_hash_header`. Populated by
    /// the handler before calling `pick()`.
    pub header_hash_value: Option<&'a str>,
    /// Value of the query parameter named by `Service.lb_hash_query_param`.
    pub query_hash_value: Option<&'a str>,
}

impl<'a> Default for RequestContext<'a> {
    fn default() -> Self {
        Self {
            client_ip: None,
            uri_path: None,
            sticky_cookie: None,
            header_hash_value: None,
            query_hash_value: None,
        }
    }
}

fn hrw_hash(key: &str, target_id: &Uuid) -> u64 {
    let mut hasher = DefaultHasher::new();
    key.hash(&mut hasher);
    target_id.hash(&mut hasher);
    hasher.finish()
}

/// Runtime state for a single target.
#[derive(Debug)]
pub struct TargetState {
    pub target: Target,
    pub in_flight: AtomicU64,
    pub healthy: AtomicBool,
}

impl TargetState {
    fn new(target: Target) -> Self {
        Self {
            target,
            in_flight: AtomicU64::new(0),
            healthy: AtomicBool::new(true),
        }
    }
}

/// RAII guard that decrements `in_flight` on drop.
pub struct InFlightGuard {
    state: Arc<TargetState>,
}

impl InFlightGuard {
    pub fn new(state: Arc<TargetState>) -> Self {
        state.in_flight.fetch_add(1, Ordering::Relaxed);
        Self { state }
    }
}

impl Drop for InFlightGuard {
    fn drop(&mut self) {
        self.state.in_flight.fetch_sub(1, Ordering::Relaxed);
    }
}

pub struct ServiceLoadBalancer {
    strategy: LbStrategy,
    targets: Vec<Arc<TargetState>>,
    /// Pre-expanded virtual ring for weighted RR. Each entry is an index into
    /// `targets`. Built once on rebuild.
    rr_ring: Vec<usize>,
    rr_counter: AtomicU64,
}

impl ServiceLoadBalancer {
    fn new(strategy: LbStrategy, targets: Vec<Target>) -> Self {
        let states: Vec<Arc<TargetState>> = targets
            .into_iter()
            .map(|t| Arc::new(TargetState::new(t)))
            .collect();

        let mut ring: Vec<usize> = Vec::new();
        for (idx, s) in states.iter().enumerate() {
            let w = s.target.weight.max(1) as usize;
            for _ in 0..w {
                ring.push(idx);
            }
        }

        Self {
            strategy,
            targets: states,
            rr_ring: ring,
            rr_counter: AtomicU64::new(0),
        }
    }

    pub fn strategy(&self) -> LbStrategy {
        self.strategy
    }

    pub fn targets(&self) -> &[Arc<TargetState>] {
        &self.targets
    }

    /// Pick a target according to the strategy. Skips disabled and unhealthy
    /// targets. Returns `None` if no target is configured or all are out.
    ///
    /// If `local_zone` is provided **and** the strategy is RoundRobin or
    /// LeastConn, the balancer first tries to find a healthy target in the
    /// same zone. Hash-based strategies skip zone preference to preserve
    /// their consistency guarantees.
    pub fn pick(&self, local_zone: Option<&str>, ctx: &RequestContext) -> Option<Arc<TargetState>> {
        if self.targets.is_empty() {
            return None;
        }

        // Zone-aware first try (only for strategies where locality makes sense).
        if let Some(zone) = local_zone {
            if matches!(
                self.strategy,
                LbStrategy::RoundRobin
                    | LbStrategy::LeastConn
                    | LbStrategy::RandomChoose
                    | LbStrategy::First
            ) {
                let zone_targets: Vec<_> = self
                    .targets
                    .iter()
                    .filter(|s| {
                        s.target.enabled
                            && s.healthy.load(Ordering::Relaxed)
                            && s.target.zone.as_deref() == Some(zone)
                    })
                    .collect();

                if !zone_targets.is_empty() {
                    return match self.strategy {
                        LbStrategy::RoundRobin => self.pick_rr_from_subset(&zone_targets),
                        LbStrategy::LeastConn => self.pick_least_conn_from_subset(&zone_targets),
                        LbStrategy::RandomChoose => {
                            self.pick_random_choose_from_subset(&zone_targets)
                        }
                        LbStrategy::First => Some(zone_targets[0].clone()),
                        _ => unreachable!(),
                    };
                }
            }
        }

        match self.strategy {
            LbStrategy::RoundRobin => self.pick_rr(),
            LbStrategy::LeastConn => self.pick_least_conn(),
            LbStrategy::IpHash => self.pick_hrw(ctx.client_ip.unwrap_or("0.0.0.0")),
            LbStrategy::ConsistentHash => self.pick_hrw(ctx.uri_path.unwrap_or("/")),
            LbStrategy::CookieSticky => self.pick_cookie_sticky(ctx.sticky_cookie),
            LbStrategy::Random => self.pick_random(),
            LbStrategy::RandomChoose => self.pick_random_choose(),
            LbStrategy::First => self.pick_first(),
            LbStrategy::HeaderHash => {
                self.pick_hrw(ctx.header_hash_value.unwrap_or("__no_header__"))
            }
            LbStrategy::QueryHash => {
                self.pick_hrw(ctx.query_hash_value.unwrap_or("__no_query__"))
            }
            LbStrategy::UriHash => self.pick_hrw(ctx.uri_path.unwrap_or("/")),
            LbStrategy::ClientIpHash => {
                self.pick_hrw(ctx.client_ip.unwrap_or("0.0.0.0"))
            }
        }
    }

    fn pick_rr_from_subset(&self, subset: &[&Arc<TargetState>]) -> Option<Arc<TargetState>> {
        let i = self.rr_counter.fetch_add(1, Ordering::Relaxed) as usize;
        Some((*subset[i % subset.len()]).clone())
    }

    fn pick_least_conn_from_subset(
        &self,
        subset: &[&Arc<TargetState>],
    ) -> Option<Arc<TargetState>> {
        let mut best: Option<&Arc<TargetState>> = None;
        let mut best_count = u64::MAX;
        for s in subset {
            let c = s.in_flight.load(Ordering::Relaxed);
            if c < best_count {
                best_count = c;
                best = Some(s);
            }
        }
        best.cloned()
    }

    fn pick_rr(&self) -> Option<Arc<TargetState>> {
        if self.rr_ring.is_empty() {
            return None;
        }
        let len = self.rr_ring.len();
        // Try up to ring length to skip unhealthy/disabled.
        for _ in 0..len {
            let i = self.rr_counter.fetch_add(1, Ordering::Relaxed) as usize;
            let idx = self.rr_ring[i % len];
            let state = &self.targets[idx];
            if state.target.enabled && state.healthy.load(Ordering::Relaxed) {
                return Some(state.clone());
            }
        }
        None
    }

    fn pick_least_conn(&self) -> Option<Arc<TargetState>> {
        let mut best: Option<&Arc<TargetState>> = None;
        let mut best_count = u64::MAX;
        for s in &self.targets {
            if !s.target.enabled || !s.healthy.load(Ordering::Relaxed) {
                continue;
            }
            let c = s.in_flight.load(Ordering::Relaxed);
            if c < best_count {
                best_count = c;
                best = Some(s);
            }
        }
        best.cloned()
    }

    /// Rendezvous / Highest-Random-Weight hashing. For each healthy target,
    /// compute hash(key, target_id) and pick the highest. Adding or removing
    /// a target only remaps requests that were pinned to that target.
    fn pick_hrw(&self, key: &str) -> Option<Arc<TargetState>> {
        let mut best: Option<&Arc<TargetState>> = None;
        let mut best_hash: u64 = 0;
        for s in &self.targets {
            if !s.target.enabled || !s.healthy.load(Ordering::Relaxed) {
                continue;
            }
            let h = hrw_hash(key, &s.target.id);
            if best.is_none() || h > best_hash {
                best_hash = h;
                best = Some(s);
            }
        }
        best.cloned()
    }

    /// Cookie sticky: honour an existing affinity cookie when the pinned
    /// target is still healthy, otherwise fall back to round-robin.
    fn pick_cookie_sticky(&self, cookie: Option<&str>) -> Option<Arc<TargetState>> {
        if let Some(val) = cookie {
            if let Ok(target_id) = Uuid::parse_str(val) {
                for s in &self.targets {
                    if s.target.id == target_id
                        && s.target.enabled
                        && s.healthy.load(Ordering::Relaxed)
                    {
                        return Some(s.clone());
                    }
                }
            }
        }
        self.pick_rr()
    }

    /// Random selection among healthy, enabled targets.
    fn pick_random(&self) -> Option<Arc<TargetState>> {
        let healthy: Vec<_> = self
            .targets
            .iter()
            .filter(|s| s.target.enabled && s.healthy.load(Ordering::Relaxed))
            .collect();
        if healthy.is_empty() {
            return None;
        }
        let idx = self.rr_counter.fetch_add(1, Ordering::Relaxed) as usize;
        // Use the counter XOR'd with a fast mix to approximate random
        // without pulling in a RNG crate — good enough for load balancing.
        let mixed = idx.wrapping_mul(6364136223846793005).wrapping_add(1);
        Some(healthy[mixed % healthy.len()].clone())
    }

    /// Power-of-2 random choices (P2C): pick 2 random healthy targets, use
    /// the one with fewer in-flight requests. Statistically near-optimal
    /// distribution without the coordination cost of full least-conn.
    fn pick_random_choose(&self) -> Option<Arc<TargetState>> {
        let healthy: Vec<_> = self
            .targets
            .iter()
            .filter(|s| s.target.enabled && s.healthy.load(Ordering::Relaxed))
            .collect();
        self.p2c_from(&healthy)
    }

    fn pick_random_choose_from_subset(
        &self,
        subset: &[&Arc<TargetState>],
    ) -> Option<Arc<TargetState>> {
        self.p2c_from(subset)
    }

    fn p2c_from(&self, pool: &[&Arc<TargetState>]) -> Option<Arc<TargetState>> {
        if pool.is_empty() {
            return None;
        }
        if pool.len() == 1 {
            return Some(pool[0].clone());
        }
        let ctr = self.rr_counter.fetch_add(2, Ordering::Relaxed) as usize;
        let mix_a = ctr.wrapping_mul(6364136223846793005).wrapping_add(1);
        let mix_b = (ctr + 1).wrapping_mul(6364136223846793005).wrapping_add(1);
        let a = pool[mix_a % pool.len()];
        let mut b_idx = mix_b % pool.len();
        // Ensure b != a when possible.
        if b_idx == mix_a % pool.len() {
            b_idx = (b_idx + 1) % pool.len();
        }
        let b = pool[b_idx];
        let fa = a.in_flight.load(Ordering::Relaxed);
        let fb = b.in_flight.load(Ordering::Relaxed);
        Some(if fa <= fb { a } else { b }.clone())
    }

    /// Primary / failover: always pick the first healthy, enabled target in
    /// list order. Traffic only moves to the next when preceding ones are down.
    fn pick_first(&self) -> Option<Arc<TargetState>> {
        self.targets
            .iter()
            .find(|s| s.target.enabled && s.healthy.load(Ordering::Relaxed))
            .cloned()
    }
}

/// Registry of per-service load balancers.
pub struct LoadBalancerRegistry {
    inner: DashMap<Uuid, Arc<RwLock<ServiceLoadBalancer>>>,
}

impl LoadBalancerRegistry {
    pub fn new() -> Self {
        Self {
            inner: DashMap::new(),
        }
    }

    pub fn rebuild_for_service(
        &self,
        service_id: Uuid,
        strategy: LbStrategy,
        targets: Vec<Target>,
    ) {
        let lb = ServiceLoadBalancer::new(strategy, targets);
        self.inner.insert(service_id, Arc::new(RwLock::new(lb)));
    }

    pub fn pick(
        &self,
        service_id: &Uuid,
        local_zone: Option<&str>,
        ctx: &RequestContext,
    ) -> Option<Arc<TargetState>> {
        let entry = self.inner.get(service_id)?;
        let guard = entry.read().ok()?;
        guard.pick(local_zone, ctx)
    }

    pub fn strategy_for(&self, service_id: &Uuid) -> Option<LbStrategy> {
        let entry = self.inner.get(service_id)?;
        let guard = entry.read().ok()?;
        Some(guard.strategy())
    }

    pub fn remove_service(&self, service_id: &Uuid) {
        self.inner.remove(service_id);
    }

    pub fn mark_target_health(&self, service_id: &Uuid, target_id: &Uuid, healthy: bool) {
        if let Some(entry) = self.inner.get(service_id) {
            if let Ok(guard) = entry.read() {
                for s in guard.targets() {
                    if s.target.id == *target_id {
                        s.healthy.store(healthy, Ordering::Relaxed);
                        break;
                    }
                }
            }
        }
    }

    /// Snapshot of all targets for a service (including runtime state).
    pub fn snapshot(&self, service_id: &Uuid) -> Vec<Arc<TargetState>> {
        match self.inner.get(service_id) {
            Some(entry) => match entry.read() {
                Ok(g) => g.targets().to_vec(),
                Err(_) => Vec::new(),
            },
            None => Vec::new(),
        }
    }
}

impl Default for LoadBalancerRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Target;
    use chrono::Utc;

    fn make_targets(n: usize) -> Vec<Target> {
        (0..n)
            .map(|i| Target {
                id: Uuid::new_v4(),
                service_id: Uuid::nil(),
                url: format!("http://host-{i}:8080"),
                weight: 1,
                enabled: true,
                zone: None,
                created_at: Utc::now(),
                updated_at: Utc::now(),
            })
            .collect()
    }

    fn make_weighted_targets(weights: &[u32]) -> Vec<Target> {
        weights
            .iter()
            .enumerate()
            .map(|(i, &w)| Target {
                id: Uuid::new_v4(),
                service_id: Uuid::nil(),
                url: format!("http://host-{i}:8080"),
                weight: w,
                enabled: true,
                zone: None,
                created_at: Utc::now(),
                updated_at: Utc::now(),
            })
            .collect()
    }

    fn default_ctx<'a>() -> RequestContext<'a> {
        RequestContext::default()
    }

    // ── RoundRobin ──

    #[test]
    fn rr_cycles_through_all_targets() {
        let targets = make_targets(3);
        let lb = ServiceLoadBalancer::new(LbStrategy::RoundRobin, targets.clone());
        let ctx = default_ctx();
        let mut seen = std::collections::HashSet::new();
        for _ in 0..6 {
            let t = lb.pick(None, &ctx).unwrap();
            seen.insert(t.target.url.clone());
        }
        assert_eq!(seen.len(), 3);
    }

    #[test]
    fn weighted_rr_respects_weights() {
        let targets = make_weighted_targets(&[3, 1]);
        let lb = ServiceLoadBalancer::new(LbStrategy::RoundRobin, targets);
        let ctx = default_ctx();
        let mut counts = std::collections::HashMap::new();
        // 4 slots in the ring: [0,0,0,1]
        for _ in 0..8 {
            let t = lb.pick(None, &ctx).unwrap();
            *counts.entry(t.target.url.clone()).or_insert(0) += 1;
        }
        // host-0 should get 3x the traffic of host-1
        assert!(*counts.get("http://host-0:8080").unwrap_or(&0) > *counts.get("http://host-1:8080").unwrap_or(&0));
    }

    #[test]
    fn rr_skips_unhealthy() {
        let targets = make_targets(3);
        let lb = ServiceLoadBalancer::new(LbStrategy::RoundRobin, targets.clone());
        // Mark target 0 unhealthy.
        lb.targets[0].healthy.store(false, Ordering::Relaxed);
        let ctx = default_ctx();
        for _ in 0..10 {
            let t = lb.pick(None, &ctx).unwrap();
            assert_ne!(t.target.url, "http://host-0:8080");
        }
    }

    #[test]
    fn rr_returns_none_when_all_unhealthy() {
        let targets = make_targets(2);
        let lb = ServiceLoadBalancer::new(LbStrategy::RoundRobin, targets);
        for ts in &lb.targets {
            ts.healthy.store(false, Ordering::Relaxed);
        }
        assert!(lb.pick(None, &default_ctx()).is_none());
    }

    // ── LeastConn ──

    #[test]
    fn least_conn_picks_lowest_inflight() {
        let targets = make_targets(3);
        let lb = ServiceLoadBalancer::new(LbStrategy::LeastConn, targets);
        // Bump in-flight on targets 0 and 1.
        lb.targets[0].in_flight.store(10, Ordering::Relaxed);
        lb.targets[1].in_flight.store(5, Ordering::Relaxed);
        lb.targets[2].in_flight.store(0, Ordering::Relaxed);
        let ctx = default_ctx();
        let picked = lb.pick(None, &ctx).unwrap();
        assert_eq!(picked.target.url, "http://host-2:8080");
    }

    // ── RandomChoose (P2C) ──

    #[test]
    fn p2c_prefers_lower_inflight() {
        let targets = make_targets(4);
        let lb = ServiceLoadBalancer::new(LbStrategy::RandomChoose, targets);
        // Make one target clearly best.
        lb.targets[0].in_flight.store(100, Ordering::Relaxed);
        lb.targets[1].in_flight.store(100, Ordering::Relaxed);
        lb.targets[2].in_flight.store(0, Ordering::Relaxed);
        lb.targets[3].in_flight.store(100, Ordering::Relaxed);
        let ctx = default_ctx();
        let mut picked_2 = 0;
        for _ in 0..100 {
            let t = lb.pick(None, &ctx).unwrap();
            if t.target.url == "http://host-2:8080" {
                picked_2 += 1;
            }
        }
        // host-2 should be picked more often than random 25%.
        assert!(picked_2 > 25, "P2C picked host-2 only {picked_2}/100 times");
    }

    #[test]
    fn p2c_single_target() {
        let targets = make_targets(1);
        let lb = ServiceLoadBalancer::new(LbStrategy::RandomChoose, targets);
        let t = lb.pick(None, &default_ctx()).unwrap();
        assert_eq!(t.target.url, "http://host-0:8080");
    }

    #[test]
    fn p2c_empty() {
        let lb = ServiceLoadBalancer::new(LbStrategy::RandomChoose, Vec::new());
        assert!(lb.pick(None, &default_ctx()).is_none());
    }

    // ── First / Failover ──

    #[test]
    fn first_always_picks_first_healthy() {
        let targets = make_targets(3);
        let lb = ServiceLoadBalancer::new(LbStrategy::First, targets);
        let ctx = default_ctx();
        for _ in 0..10 {
            let t = lb.pick(None, &ctx).unwrap();
            assert_eq!(t.target.url, "http://host-0:8080");
        }
    }

    #[test]
    fn first_failover_when_primary_down() {
        let targets = make_targets(3);
        let lb = ServiceLoadBalancer::new(LbStrategy::First, targets);
        lb.targets[0].healthy.store(false, Ordering::Relaxed);
        let ctx = default_ctx();
        let t = lb.pick(None, &ctx).unwrap();
        assert_eq!(t.target.url, "http://host-1:8080");
    }

    #[test]
    fn first_failover_cascading() {
        let targets = make_targets(3);
        let lb = ServiceLoadBalancer::new(LbStrategy::First, targets);
        lb.targets[0].healthy.store(false, Ordering::Relaxed);
        lb.targets[1].healthy.store(false, Ordering::Relaxed);
        let t = lb.pick(None, &default_ctx()).unwrap();
        assert_eq!(t.target.url, "http://host-2:8080");
    }

    #[test]
    fn first_none_when_all_down() {
        let targets = make_targets(2);
        let lb = ServiceLoadBalancer::new(LbStrategy::First, targets);
        for ts in &lb.targets {
            ts.healthy.store(false, Ordering::Relaxed);
        }
        assert!(lb.pick(None, &default_ctx()).is_none());
    }

    // ── HeaderHash ──

    #[test]
    fn header_hash_consistent() {
        let targets = make_targets(5);
        let lb = ServiceLoadBalancer::new(LbStrategy::HeaderHash, targets);
        let ctx = RequestContext {
            header_hash_value: Some("tenant-42"),
            ..Default::default()
        };
        let first = lb.pick(None, &ctx).unwrap();
        for _ in 0..20 {
            let t = lb.pick(None, &ctx).unwrap();
            assert_eq!(t.target.id, first.target.id, "header hash not consistent");
        }
    }

    #[test]
    fn header_hash_different_values_spread() {
        let targets = make_targets(4);
        let lb = ServiceLoadBalancer::new(LbStrategy::HeaderHash, targets);
        let mut seen = std::collections::HashSet::new();
        for i in 0..20 {
            let val = format!("tenant-{i}");
            let ctx = RequestContext {
                header_hash_value: Some(&val),
                ..Default::default()
            };
            let t = lb.pick(None, &ctx).unwrap();
            seen.insert(t.target.url.clone());
        }
        // With 20 distinct values and 4 targets, expect > 1 distinct target.
        assert!(seen.len() > 1, "header hash didn't spread: {:?}", seen);
    }

    // ── QueryHash ──

    #[test]
    fn query_hash_consistent() {
        let targets = make_targets(3);
        let lb = ServiceLoadBalancer::new(LbStrategy::QueryHash, targets);
        let ctx = RequestContext {
            query_hash_value: Some("user_99"),
            ..Default::default()
        };
        let first = lb.pick(None, &ctx).unwrap();
        for _ in 0..20 {
            let t = lb.pick(None, &ctx).unwrap();
            assert_eq!(t.target.id, first.target.id);
        }
    }

    // ── Zone-aware subset ──

    #[test]
    fn zone_aware_p2c() {
        let mut targets = make_targets(4);
        targets[0].zone = Some("us-east-1a".to_string());
        targets[1].zone = Some("us-east-1a".to_string());
        targets[2].zone = Some("us-east-1b".to_string());
        targets[3].zone = Some("us-east-1b".to_string());
        let lb = ServiceLoadBalancer::new(LbStrategy::RandomChoose, targets);
        let ctx = default_ctx();
        // Zone-aware should prefer 1a targets.
        for _ in 0..20 {
            let t = lb.pick(Some("us-east-1a"), &ctx).unwrap();
            assert!(
                t.target.url == "http://host-0:8080" || t.target.url == "http://host-1:8080",
                "expected zone-a target, got {}",
                t.target.url
            );
        }
    }

    #[test]
    fn zone_aware_first() {
        let mut targets = make_targets(3);
        targets[0].zone = Some("zone-b".to_string());
        targets[1].zone = Some("zone-a".to_string());
        targets[2].zone = Some("zone-a".to_string());
        let lb = ServiceLoadBalancer::new(LbStrategy::First, targets);
        let t = lb.pick(Some("zone-a"), &default_ctx()).unwrap();
        assert_eq!(t.target.url, "http://host-1:8080");
    }

    // ── LbStrategy::from_str ──

    #[test]
    fn strategy_parsing() {
        assert_eq!(LbStrategy::from_str("random-choose"), LbStrategy::RandomChoose);
        assert_eq!(LbStrategy::from_str("p2c"), LbStrategy::RandomChoose);
        assert_eq!(LbStrategy::from_str("first"), LbStrategy::First);
        assert_eq!(LbStrategy::from_str("failover"), LbStrategy::First);
        assert_eq!(LbStrategy::from_str("header-hash"), LbStrategy::HeaderHash);
        assert_eq!(LbStrategy::from_str("header_hash"), LbStrategy::HeaderHash);
        assert_eq!(LbStrategy::from_str("query-hash"), LbStrategy::QueryHash);
        assert_eq!(LbStrategy::from_str("query_hash"), LbStrategy::QueryHash);
        assert_eq!(LbStrategy::from_str("unknown-thing"), LbStrategy::RoundRobin);
    }

    // ── InFlightGuard ──

    #[test]
    fn in_flight_guard_increments_and_decrements() {
        let targets = make_targets(1);
        let lb = ServiceLoadBalancer::new(LbStrategy::First, targets);
        let ts = lb.targets[0].clone();
        assert_eq!(ts.in_flight.load(Ordering::Relaxed), 0);
        {
            let _g = InFlightGuard::new(ts.clone());
            assert_eq!(ts.in_flight.load(Ordering::Relaxed), 1);
            {
                let _g2 = InFlightGuard::new(ts.clone());
                assert_eq!(ts.in_flight.load(Ordering::Relaxed), 2);
            }
            assert_eq!(ts.in_flight.load(Ordering::Relaxed), 1);
        }
        assert_eq!(ts.in_flight.load(Ordering::Relaxed), 0);
    }

    // ── Registry ──

    #[test]
    fn registry_snapshot_includes_state() {
        let reg = LoadBalancerRegistry::new();
        let sid = Uuid::new_v4();
        let targets = make_targets(2);
        let tid = targets[0].id;
        reg.rebuild_for_service(sid, LbStrategy::RoundRobin, targets);
        reg.mark_target_health(&sid, &tid, false);
        let snap = reg.snapshot(&sid);
        assert_eq!(snap.len(), 2);
        let t0 = snap.iter().find(|s| s.target.id == tid).unwrap();
        assert!(!t0.healthy.load(Ordering::Relaxed));
    }

    // ── Feature 86: UriHash ──

    #[test]
    fn uri_hash_consistent_for_same_path() {
        let targets = make_targets(5);
        let lb = ServiceLoadBalancer::new(LbStrategy::UriHash, targets);
        let ctx = RequestContext {
            uri_path: Some("/api/v1/users"),
            ..Default::default()
        };
        let first = lb.pick(None, &ctx).unwrap();
        for _ in 0..20 {
            let t = lb.pick(None, &ctx).unwrap();
            assert_eq!(t.target.id, first.target.id, "uri_hash not consistent for same path");
        }
    }

    #[test]
    fn uri_hash_different_paths_spread() {
        let targets = make_targets(4);
        let lb = ServiceLoadBalancer::new(LbStrategy::UriHash, targets);
        let mut seen = std::collections::HashSet::new();
        for i in 0..20 {
            let path = format!("/path/{i}");
            let ctx = RequestContext {
                uri_path: Some(&path),
                ..Default::default()
            };
            let t = lb.pick(None, &ctx).unwrap();
            seen.insert(t.target.url.clone());
        }
        assert!(seen.len() > 1, "uri_hash didn't spread across targets: {:?}", seen);
    }

    // ── Feature 87: ClientIpHash ──

    #[test]
    fn client_ip_hash_consistent() {
        let targets = make_targets(5);
        let lb = ServiceLoadBalancer::new(LbStrategy::ClientIpHash, targets);
        let ctx = RequestContext {
            client_ip: Some("203.0.113.42"),
            ..Default::default()
        };
        let first = lb.pick(None, &ctx).unwrap();
        for _ in 0..20 {
            let t = lb.pick(None, &ctx).unwrap();
            assert_eq!(t.target.id, first.target.id, "client_ip_hash not consistent");
        }
    }

    #[test]
    fn client_ip_hash_different_ips_spread() {
        let targets = make_targets(4);
        let lb = ServiceLoadBalancer::new(LbStrategy::ClientIpHash, targets);
        let mut seen = std::collections::HashSet::new();
        for i in 0..20 {
            let ip = format!("10.0.0.{i}");
            let ctx = RequestContext {
                client_ip: Some(&ip),
                ..Default::default()
            };
            let t = lb.pick(None, &ctx).unwrap();
            seen.insert(t.target.url.clone());
        }
        assert!(seen.len() > 1, "client_ip_hash didn't spread: {:?}", seen);
    }

    #[test]
    fn strategy_parsing_uri_and_client_ip() {
        assert_eq!(LbStrategy::from_str("uri-hash"), LbStrategy::UriHash);
        assert_eq!(LbStrategy::from_str("uri_hash"), LbStrategy::UriHash);
        assert_eq!(LbStrategy::from_str("urihash"), LbStrategy::UriHash);
        assert_eq!(LbStrategy::from_str("client-ip-hash"), LbStrategy::ClientIpHash);
        assert_eq!(LbStrategy::from_str("client_ip_hash"), LbStrategy::ClientIpHash);
    }
}
