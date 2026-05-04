use chrono::{DateTime, Utc};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HealthStatus {
    Healthy,
    Unhealthy,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CircuitState {
    Closed,
    Open,
    HalfOpen,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceHealth {
    pub service_id: Uuid,
    pub status: HealthStatus,
    pub circuit: CircuitState,
    pub consecutive_failures: u32,
    pub consecutive_successes: u32,
    pub last_check_at: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
    pub total_checks: u64,
    pub total_failures: u64,
    pub opened_at: Option<DateTime<Utc>>,
}

impl ServiceHealth {
    fn new(service_id: Uuid) -> Self {
        Self {
            service_id,
            status: HealthStatus::Unknown,
            circuit: CircuitState::Closed,
            consecutive_failures: 0,
            consecutive_successes: 0,
            last_check_at: None,
            last_error: None,
            total_checks: 0,
            total_failures: 0,
            opened_at: None,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct HealthConfig {
    pub failure_threshold: u32,
    pub success_threshold: u32,
    pub open_duration_ms: u64,
}

impl Default for HealthConfig {
    fn default() -> Self {
        Self {
            failure_threshold: 3,
            success_threshold: 2,
            open_duration_ms: 30_000,
        }
    }
}

pub struct HealthRegistry {
    entries: DashMap<Uuid, ServiceHealth>,
    config: HealthConfig,
}

impl HealthRegistry {
    pub fn new() -> Self {
        Self::with_config(HealthConfig::default())
    }

    pub fn with_config(config: HealthConfig) -> Self {
        Self {
            entries: DashMap::new(),
            config,
        }
    }

    pub fn config(&self) -> HealthConfig {
        self.config
    }

    pub fn register(&self, service_id: Uuid) {
        self.entries
            .entry(service_id)
            .or_insert_with(|| ServiceHealth::new(service_id));
    }

    pub fn unregister(&self, service_id: &Uuid) {
        self.entries.remove(service_id);
    }

    pub fn get(&self, service_id: &Uuid) -> Option<ServiceHealth> {
        self.entries.get(service_id).map(|e| e.clone())
    }

    pub fn list(&self) -> Vec<ServiceHealth> {
        self.entries.iter().map(|e| e.clone()).collect()
    }

    pub fn record_success(&self, service_id: &Uuid, service_name: &str) {
        let success_threshold = self.config.success_threshold;
        let (circuit, status) = {
            let mut entry = self
                .entries
                .entry(*service_id)
                .or_insert_with(|| ServiceHealth::new(*service_id));

            entry.status = HealthStatus::Healthy;
            entry.consecutive_failures = 0;
            entry.consecutive_successes = entry.consecutive_successes.saturating_add(1);
            entry.last_check_at = Some(Utc::now());
            entry.last_error = None;
            entry.total_checks = entry.total_checks.saturating_add(1);

            if entry.circuit == CircuitState::HalfOpen
                && entry.consecutive_successes >= success_threshold
            {
                tracing::info!(service_id = %service_id, "circuit breaker closed");
                entry.circuit = CircuitState::Closed;
                entry.opened_at = None;
            }

            (entry.circuit, entry.status)
        };

        metrics::gauge!("lynxx_circuit_state", "service" => service_name.to_string())
            .set(crate::metrics::circuit_state_value(circuit));
        metrics::gauge!("lynxx_service_health", "service" => service_name.to_string())
            .set(crate::metrics::health_status_value(status));
    }

    pub fn record_failure(&self, service_id: &Uuid, service_name: &str, err: String) {
        let failure_threshold = self.config.failure_threshold;
        let (circuit, status, opened_transition) = {
            let mut entry = self
                .entries
                .entry(*service_id)
                .or_insert_with(|| ServiceHealth::new(*service_id));

            entry.status = HealthStatus::Unhealthy;
            entry.consecutive_successes = 0;
            entry.consecutive_failures = entry.consecutive_failures.saturating_add(1);
            entry.last_check_at = Some(Utc::now());
            entry.last_error = Some(err);
            entry.total_checks = entry.total_checks.saturating_add(1);
            entry.total_failures = entry.total_failures.saturating_add(1);

            let mut transitioned_to_open = false;
            match entry.circuit {
                CircuitState::Closed if entry.consecutive_failures >= failure_threshold => {
                    tracing::warn!(
                        service_id = %service_id,
                        failures = entry.consecutive_failures,
                        "circuit breaker opened"
                    );
                    entry.circuit = CircuitState::Open;
                    entry.opened_at = Some(Utc::now());
                    transitioned_to_open = true;
                }
                CircuitState::HalfOpen => {
                    tracing::warn!(service_id = %service_id, "circuit breaker re-opened from half-open");
                    entry.circuit = CircuitState::Open;
                    entry.opened_at = Some(Utc::now());
                }
                _ => {}
            }

            (entry.circuit, entry.status, transitioned_to_open)
        };

        if opened_transition {
            metrics::counter!(
                "lynxx_circuit_open_total",
                "service" => service_name.to_string()
            )
            .increment(1);
        }

        metrics::gauge!("lynxx_circuit_state", "service" => service_name.to_string())
            .set(crate::metrics::circuit_state_value(circuit));
        metrics::gauge!("lynxx_service_health", "service" => service_name.to_string())
            .set(crate::metrics::health_status_value(status));
    }

    pub fn should_allow_request(&self, service_id: &Uuid, open_duration_ms: u64) -> bool {
        let mut entry = match self.entries.get_mut(service_id) {
            Some(e) => e,
            None => return true,
        };

        match entry.circuit {
            CircuitState::Closed => true,
            CircuitState::HalfOpen => true,
            CircuitState::Open => {
                let elapsed_ok = entry
                    .opened_at
                    .map(|t| {
                        Utc::now().signed_duration_since(t).num_milliseconds() as u64
                            > open_duration_ms
                    })
                    .unwrap_or(true);
                if elapsed_ok {
                    tracing::info!(service_id = %service_id, "circuit breaker half-open (probing)");
                    entry.circuit = CircuitState::HalfOpen;
                    entry.consecutive_successes = 0;
                    true
                } else {
                    false
                }
            }
        }
    }
}

impl Default for HealthRegistry {
    fn default() -> Self {
        Self::new()
    }
}
