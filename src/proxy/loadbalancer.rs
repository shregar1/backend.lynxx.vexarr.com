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
}

impl LbStrategy {
    pub fn from_str(s: &str) -> Self {
        match s {
            "least-conn" | "least_conn" | "leastconn" => LbStrategy::LeastConn,
            "ip-hash" | "ip_hash" | "iphash" => LbStrategy::IpHash,
            "cookie" | "cookie-sticky" | "sticky" => LbStrategy::CookieSticky,
            "consistent-hash" | "consistent_hash" | "consistenthash" => LbStrategy::ConsistentHash,
            _ => LbStrategy::RoundRobin,
        }
    }
}

/// Request-scoped context for hash-based LB strategies.
pub struct RequestContext<'a> {
    pub client_ip: Option<&'a str>,
    pub uri_path: Option<&'a str>,
    pub sticky_cookie: Option<&'a str>,
}

impl<'a> Default for RequestContext<'a> {
    fn default() -> Self {
        Self {
            client_ip: None,
            uri_path: None,
            sticky_cookie: None,
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
            if matches!(self.strategy, LbStrategy::RoundRobin | LbStrategy::LeastConn) {
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
