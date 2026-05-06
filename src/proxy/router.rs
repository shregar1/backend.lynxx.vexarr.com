use crate::db::Repository;
use crate::error::GatewayError;
use crate::models::{Route, Service};
use dashmap::DashMap;
use std::sync::Arc;
use uuid::Uuid;

pub struct RouteTable {
    services: DashMap<Uuid, Service>,
    routes: DashMap<Uuid, Route>,
    /// Exact-path index: `path` -> `route_id` for routes whose `paths` entries
    /// are literal (no trailing `*`). The hot path consults this first to turn
    /// route matching from O(N) into O(1) for the common "exact path" case.
    /// Prefix and wildcard routes still fall through to the linear scan.
    exact_index: DashMap<String, Uuid>,
}

impl RouteTable {
    pub fn new() -> Self {
        Self {
            services: DashMap::new(),
            routes: DashMap::new(),
            exact_index: DashMap::new(),
        }
    }

    fn rebuild_exact_index(&self) {
        self.exact_index.clear();
        for entry in self.routes.iter() {
            let route = entry.value();
            for p in &route.paths {
                if !p.ends_with('*') {
                    self.exact_index.insert(p.clone(), route.id);
                }
            }
        }
    }

    /// Bulk-load services and routes from the persistent repository,
    /// replacing whatever is currently in memory.
    pub async fn load_from_repo(
        &self,
        repo: &dyn Repository,
    ) -> Result<(usize, usize), GatewayError> {
        let services = repo.list_services().await?;
        let routes = repo.list_routes().await?;

        self.services.clear();
        self.routes.clear();

        for svc in &services {
            self.services.insert(svc.id, svc.clone());
        }
        for route in &routes {
            self.routes.insert(route.id, route.clone());
        }
        self.rebuild_exact_index();

        Ok((services.len(), routes.len()))
    }

    pub fn add_service(&self, service: Service) -> Uuid {
        let id = service.id;
        self.services.insert(id, service);
        id
    }

    pub fn get_service(&self, id: &Uuid) -> Option<Service> {
        self.services.get(id).map(|s| s.clone())
    }

    pub fn get_service_by_name(&self, name: &str) -> Option<Service> {
        self.services
            .iter()
            .find(|s| s.name == name)
            .map(|s| s.clone())
    }

    pub fn list_services(&self) -> Vec<Service> {
        self.services.iter().map(|s| s.clone()).collect()
    }

    pub fn update_service(&self, id: &Uuid, service: Service) -> bool {
        if self.services.contains_key(id) {
            self.services.insert(*id, service);
            true
        } else {
            false
        }
    }

    pub fn remove_service(&self, id: &Uuid) -> bool {
        self.services.remove(id).is_some()
    }

    pub fn add_route(&self, route: Route) -> Uuid {
        let id = route.id;
        self.routes.insert(id, route);
        self.rebuild_exact_index();
        id
    }

    pub fn get_route(&self, id: &Uuid) -> Option<Route> {
        self.routes.get(id).map(|r| r.clone())
    }

    pub fn list_routes(&self) -> Vec<Route> {
        self.routes.iter().map(|r| r.clone()).collect()
    }

    pub fn update_route(&self, id: &Uuid, route: Route) -> bool {
        if self.routes.contains_key(id) {
            self.routes.insert(*id, route);
            self.rebuild_exact_index();
            true
        } else {
            false
        }
    }

    pub fn remove_route(&self, id: &Uuid) -> bool {
        let removed = self.routes.remove(id).is_some();
        if removed {
            self.rebuild_exact_index();
        }
        removed
    }

    pub fn match_route(
        &self,
        path: &str,
        method: &str,
        host: Option<&str>,
    ) -> Option<(Route, Service)> {
        // Fast path: exact-path lookup. Avoids the O(N) scan for the common
        // case of a busy API with literal route paths.
        if let Some(id_ref) = self.exact_index.get(path) {
            let id = *id_ref;
            drop(id_ref);
            if let Some(route_ref) = self.routes.get(&id) {
                let route = route_ref.value();
                let method_matches =
                    route.methods.is_empty() || route.methods.iter().any(|m| m == method);
                let host_matches = route.hosts.is_empty()
                    || host.map_or(false, |h| {
                        route.hosts.iter().any(|rh| rh.eq_ignore_ascii_case(h))
                    });
                if method_matches && host_matches {
                    if let Some(service) = self.get_service(&route.service_id) {
                        return Some((route.clone(), service));
                    }
                }
            }
        }

        // Prefer routes with explicit host constraints over wildcard ones.
        let mut best: Option<(Route, Service)> = None;
        let mut best_has_hosts = false;

        for entry in self.routes.iter() {
            let route = entry.value();

            let path_matches = route.paths.iter().any(|p| {
                if p.ends_with('*') {
                    let prefix = &p[..p.len() - 1];
                    path.starts_with(prefix)
                } else {
                    path == p || path.starts_with(&format!("{p}/"))
                }
            });

            if !path_matches {
                continue;
            }

            let method_matches =
                route.methods.is_empty() || route.methods.iter().any(|m| m == method);

            if !method_matches {
                continue;
            }

            let host_matches = route.hosts.is_empty()
                || host.map_or(false, |h| {
                    route.hosts.iter().any(|rh| rh.eq_ignore_ascii_case(h))
                });

            if !host_matches {
                continue;
            }

            let has_hosts = !route.hosts.is_empty();
            if has_hosts && !best_has_hosts {
                best = Some((route.clone(), self.get_service(&route.service_id)?));
                best_has_hosts = true;
            } else if best.is_none() {
                if let Some(service) = self.get_service(&route.service_id) {
                    best = Some((route.clone(), service));
                    best_has_hosts = has_hosts;
                }
            }
        }

        best
    }

    pub fn resolve_upstream_path(&self, route: &Route, original_path: &str) -> String {
        if route.strip_path {
            for prefix in &route.paths {
                let prefix = prefix.trim_end_matches('*');
                if original_path.starts_with(prefix) {
                    let remaining = &original_path[prefix.len()..];
                    if remaining.is_empty() || remaining.starts_with('/') {
                        return remaining.to_string();
                    }
                    return format!("/{remaining}");
                }
            }
        }
        original_path.to_string()
    }
}

impl Default for RouteTable {
    fn default() -> Self {
        Self::new()
    }
}

pub type SharedRouteTable = Arc<RouteTable>;
