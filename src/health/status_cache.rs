use crate::admin::routes::StatusResponse;
use crate::db::Repository;
use crate::proxy::router::SharedRouteTable;
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio::time::{self, Duration};

#[derive(Clone)]
pub struct StatusCache {
    cached: Arc<RwLock<StatusResponse>>,
}

impl StatusCache {
    pub fn new(initial: StatusResponse) -> Self {
        Self {
            cached: Arc::new(RwLock::new(initial)),
        }
    }

    pub async fn get(&self) -> StatusResponse {
        self.cached.read().await.clone()
    }

    async fn update(&self, response: StatusResponse) {
        *self.cached.write().await = response;
    }
}

pub fn spawn_status_updater(
    cache: StatusCache,
    route_table: SharedRouteTable,
    repo: Arc<dyn Repository>,
    license_state: Option<crate::license::LicenseState>,
    license_api_key: String,
    start_time: chrono::DateTime<chrono::Utc>,
    interval_ms: u64,
) {
    tokio::spawn(async move {
        let mut ticker = time::interval(Duration::from_millis(interval_ms));
        ticker.set_missed_tick_behavior(time::MissedTickBehavior::Skip);

        loop {
            ticker.tick().await;

            let response = build_status(
                &route_table,
                &repo,
                &license_state,
                &license_api_key,
                start_time,
            )
            .await;

            cache.update(response).await;
        }
    });
}

pub async fn build_status(
    route_table: &SharedRouteTable,
    repo: &Arc<dyn Repository>,
    license_state: &Option<crate::license::LicenseState>,
    license_api_key: &str,
    start_time: chrono::DateTime<chrono::Utc>,
) -> StatusResponse {
    let uptime = chrono::Utc::now()
        .signed_duration_since(start_time)
        .num_seconds();

    let plugins_count = repo.list_plugins().await.map(|p| p.len()).unwrap_or(0);
    let consumers_count = repo.list_consumers().await.map(|c| c.len()).unwrap_or(0);

    let (tier, active_nodes, max_nodes) = match license_state {
        Some(ls) => {
            let t = ls.tier();
            (t.as_str().to_string(), ls.active_nodes(), ls.max_nodes())
        }
        None => ("unknown".into(), 0, None),
    };

    let instance_id = crate::license::compute_instance_id(license_api_key);

    StatusResponse {
        status: "running".into(),
        version: env!("CARGO_PKG_VERSION").into(),
        build_hash: env!("LYNXX_BUILD_HASH").into(),
        uptime_seconds: uptime,
        services_count: route_table.list_services().len(),
        routes_count: route_table.list_routes().len(),
        plugins_count,
        consumers_count,
        tier,
        active_nodes,
        max_nodes,
        instance_id,
    }
}
