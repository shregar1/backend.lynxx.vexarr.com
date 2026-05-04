use crate::db::Repository;
use crate::error::GatewayError;
use crate::models::{Route, Service};
use dashmap::DashMap;
use std::sync::Arc;
use uuid::Uuid;

pub struct RouteTable {
    services: DashMap<Uuid, Service>,
    routes: DashMap<Uuid, Route>,
}

impl RouteTable {
    pub fn new() -> Self {
        Self {
            services: DashMap::new(),
            routes: DashMap::new(),
        }
    }

    /// Bulk-load services and routes from the persistent repository,
    /// replacing whatever is currently in memory.
    pub async fn load_from_repo(&self, repo: &Repository) -> Result<(usize, usize), GatewayError> {
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
            true
        } else {
            false
        }
    }

    pub fn remove_route(&self, id: &Uuid) -> bool {
        self.routes.remove(id).is_some()
    }

    pub fn match_route(&self, path: &str, method: &str) -> Option<(Route, Service)> {
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

            if let Some(service) = self.get_service(&route.service_id) {
                return Some((route.clone(), service));
            }
        }

        None
    }

    pub fn resolve_upstream_path(
        &self,
        route: &Route,
        original_path: &str,
    ) -> String {
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
