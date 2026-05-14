pub mod abstraction;

// Re-export admin state and all controller modules
pub use crate::admin::routes::AdminState;
pub use crate::admin::admin_router;

// Re-export all handler modules as controller modules
pub use crate::admin::cache as cache_controller;
pub use crate::admin::cluster_mgmt as cluster_controller;
pub use crate::admin::cost as cost_controller;
pub use crate::admin::incidents as incidents_controller;
pub use crate::admin::loadtest as loadtest_controller;
pub use crate::admin::marketplace as marketplace_controller;
pub use crate::admin::pki as pki_controller;
pub use crate::admin::ratelimit_sim as ratelimit_controller;
pub use crate::admin::tracing_api as tracing_controller;
pub use crate::admin::transforms as transforms_controller;
pub use crate::admin::versioning as versioning_controller;
pub use crate::admin::webhooks as webhooks_controller;
pub use crate::admin::alerts as alerts_controller;
pub use crate::admin::analytics as analytics_controller;
pub use crate::admin::consumer_groups as consumer_groups_controller;
pub use crate::admin::deploy as deploy_controller;
pub use crate::admin::docs as docs_controller;
pub use crate::admin::error_pages as error_pages_controller;
pub use crate::admin::graph as graph_controller;
pub use crate::admin::ip_rules as ip_rules_controller;
pub use crate::admin::probes as probes_controller;
pub use crate::admin::profiling as profiling_controller;
pub use crate::admin::replay as replay_controller;
pub use crate::admin::snapshots as snapshots_controller;
