//! Prometheus metrics for the Lynxx gateway.
//!
//! Installs a global recorder via `metrics-exporter-prometheus` and exposes a
//! handle that the admin router uses to render `/metrics`.

use crate::db::Repository;
use crate::health::{CircuitState, HealthRegistry, HealthStatus};
use crate::proxy::router::SharedRouteTable;
use metrics_exporter_prometheus::{Matcher, PrometheusBuilder, PrometheusHandle};
use std::sync::Arc;
use std::time::Duration;

/// Install the global Prometheus recorder and return the rendering handle.
pub fn install() -> PrometheusHandle {
    let buckets: [f64; 11] = [
        0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0,
    ];

    PrometheusBuilder::new()
        .set_buckets_for_metric(
            Matcher::Full("lynxx_request_duration_seconds".to_string()),
            &buckets,
        )
        .expect("valid histogram buckets")
        .install_recorder()
        .expect("failed to install Prometheus recorder")
}

/// Register HELP text for every metric we emit. Call once at startup, after
/// `install()`.
pub fn describe() {
    metrics::describe_counter!(
        "lynxx_requests_total",
        "Total number of proxy requests, labeled by service, route, method and status bucket."
    );
    metrics::describe_counter!(
        "lynxx_upstream_errors_total",
        "Upstream call failures, labeled by service and error kind (timeout|connect|5xx|other)."
    );
    metrics::describe_counter!(
        "lynxx_circuit_open_total",
        "Number of times a service circuit transitioned from Closed to Open."
    );
    metrics::describe_counter!(
        "lynxx_health_checks_total",
        "Active health checks performed, labeled by service and result (success|failure)."
    );
    metrics::describe_histogram!(
        "lynxx_request_duration_seconds",
        "End-to-end proxy request duration in seconds."
    );
    metrics::describe_gauge!(
        "lynxx_circuit_state",
        "Current circuit breaker state per service: 0=Closed, 1=HalfOpen, 2=Open."
    );
    metrics::describe_gauge!(
        "lynxx_service_health",
        "Current health per service: 0=Healthy, 1=Unhealthy, 2=Unknown."
    );
    metrics::describe_gauge!(
        "lynxx_target_health",
        "Per-target health: 0=Healthy, 1=Unhealthy."
    );
    metrics::describe_gauge!(
        "lynxx_services_total",
        "Number of configured services."
    );
    metrics::describe_gauge!("lynxx_routes_total", "Number of configured routes.");
    metrics::describe_gauge!("lynxx_plugins_total", "Number of configured plugins.");
    metrics::describe_gauge!(
        "lynxx_consumers_total",
        "Number of configured consumers."
    );
}

/// Bucket an HTTP status code into a Prometheus label value.
pub fn status_bucket(status: u16) -> &'static str {
    match status {
        100..=199 => "1xx",
        200..=299 => "2xx",
        300..=399 => "3xx",
        400..=499 => "4xx",
        500..=599 => "5xx",
        _ => "other",
    }
}

pub fn circuit_state_value(state: CircuitState) -> f64 {
    match state {
        CircuitState::Closed => 0.0,
        CircuitState::HalfOpen => 1.0,
        CircuitState::Open => 2.0,
    }
}

pub fn health_status_value(status: HealthStatus) -> f64 {
    match status {
        HealthStatus::Healthy => 0.0,
        HealthStatus::Unhealthy => 1.0,
        HealthStatus::Unknown => 2.0,
    }
}

/// Spawn a background task that periodically refreshes the slow-moving gauges
/// (object counts). Per-service health/circuit gauges are updated synchronously
/// inside the registry.
pub fn spawn_metrics_collector(
    route_table: SharedRouteTable,
    repo: Arc<Repository>,
    registry: Arc<HealthRegistry>,
    interval_ms: u64,
) {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_millis(interval_ms));
        loop {
            ticker.tick().await;

            let services = route_table.list_services();
            metrics::gauge!("lynxx_services_total").set(services.len() as f64);
            metrics::gauge!("lynxx_routes_total")
                .set(route_table.list_routes().len() as f64);

            match repo.list_plugins().await {
                Ok(plugins) => {
                    metrics::gauge!("lynxx_plugins_total").set(plugins.len() as f64);
                }
                Err(e) => tracing::warn!("metrics: failed to list plugins: {e}"),
            }

            match repo.list_consumers().await {
                Ok(consumers) => {
                    metrics::gauge!("lynxx_consumers_total")
                        .set(consumers.len() as f64);
                }
                Err(e) => tracing::warn!("metrics: failed to list consumers: {e}"),
            }

            // Refresh per-service health/circuit gauges with the canonical
            // service name as the label.
            for svc in &services {
                if let Some(h) = registry.get(&svc.id) {
                    metrics::gauge!("lynxx_circuit_state", "service" => svc.name.clone())
                        .set(circuit_state_value(h.circuit));
                    metrics::gauge!("lynxx_service_health", "service" => svc.name.clone())
                        .set(health_status_value(h.status));
                }
            }
        }
    });
}
