use crate::db::Repository;
use crate::health::registry::HealthRegistry;
use crate::proxy::loadbalancer::LoadBalancerRegistry;
use crate::proxy::router::SharedRouteTable;
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

pub fn spawn_health_checker(
    route_table: SharedRouteTable,
    registry: Arc<HealthRegistry>,
    repo: Arc<Repository>,
    lb: Arc<LoadBalancerRegistry>,
    client: reqwest::Client,
    interval_ms: u64,
) {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_millis(interval_ms));
        // Skip the immediate first tick.
        ticker.tick().await;

        loop {
            ticker.tick().await;
            run_once(&route_table, &registry, &repo, &lb, &client).await;
        }
    });
}

async fn run_once(
    route_table: &SharedRouteTable,
    registry: &Arc<HealthRegistry>,
    repo: &Arc<Repository>,
    lb: &Arc<LoadBalancerRegistry>,
    client: &reqwest::Client,
) {
    let services = route_table.list_services();
    let mut current_ids = HashSet::new();

    for svc in &services {
        current_ids.insert(svc.id);
        registry.register(svc.id);

        let url = format!("{}/health", svc.url.trim_end_matches('/'));
        let req = client
            .get(&url)
            .timeout(Duration::from_secs(5))
            .send()
            .await;

        match req {
            Ok(resp) if resp.status().is_success() => {
                registry.record_success(&svc.id, &svc.name);
                metrics::counter!(
                    "lynxx_health_checks_total",
                    "service" => svc.name.clone(),
                    "result" => "success",
                )
                .increment(1);
            }
            Ok(resp) => {
                registry.record_failure(
                    &svc.id,
                    &svc.name,
                    format!("health check non-2xx: {}", resp.status()),
                );
                metrics::counter!(
                    "lynxx_health_checks_total",
                    "service" => svc.name.clone(),
                    "result" => "failure",
                )
                .increment(1);
            }
            Err(e) => {
                registry.record_failure(
                    &svc.id,
                    &svc.name,
                    format!("health check error: {e}"),
                );
                metrics::counter!(
                    "lynxx_health_checks_total",
                    "service" => svc.name.clone(),
                    "result" => "failure",
                )
                .increment(1);
            }
        }

        // Per-target health checks.
        let targets = match repo.list_targets_for_service(&svc.id).await {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!(service = %svc.name, "failed to list targets: {e}");
                continue;
            }
        };

        for target in &targets {
            if !target.enabled {
                continue;
            }
            let turl = format!("{}/health", target.url.trim_end_matches('/'));
            let resp = client
                .get(&turl)
                .timeout(Duration::from_secs(5))
                .send()
                .await;

            let healthy = matches!(&resp, Ok(r) if r.status().is_success());
            lb.mark_target_health(&svc.id, &target.id, healthy);

            metrics::gauge!(
                "lynxx_target_health",
                "service" => svc.name.clone(),
                "target" => target.url.clone(),
            )
            .set(if healthy { 0.0 } else { 1.0 });
        }
    }

    // Drop entries for services that no longer exist.
    let stale: Vec<_> = registry
        .list()
        .into_iter()
        .map(|h| h.service_id)
        .filter(|id| !current_ids.contains(id))
        .collect();

    for id in stale {
        tracing::info!(service_id = %id, "removing stale health entry");
        registry.unregister(&id);
    }
}
