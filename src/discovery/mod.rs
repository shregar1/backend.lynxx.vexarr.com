//! Service-discovery providers.
//!
//! Out of the box Lynxx loads services + routes from `config.toml` and
//! its admin API. Providers extend that with *external* sources of
//! truth that the gateway watches and reconciles into its own
//! repository — same shape Traefik uses (`providers.docker`,
//! `providers.kubernetes`, etc.).
//!
//! Today only the Docker provider is implemented. Each provider:
//!
//! 1. Builds a fresh snapshot of `(services, routes)` from its source
//!    (Docker labels, K8s annotations, etc.).
//! 2. Diffs against the existing repository state.
//! 3. Calls the repository's CRUD methods to converge.
//! 4. Watches for change events and repeats.
//!
//! Provider-managed entities are tagged via the service `name` prefix
//! (`docker:my-app` etc.) so they're recognisable in the admin UI and
//! distinguishable from operator-created entries.

pub mod consul;
pub mod docker;
pub mod docker_swarm;
pub mod ecs;
pub mod etcd;
pub mod http_provider;
pub mod nomad;
pub mod redis_kv;
pub mod srv_dns;
pub mod zookeeper;
