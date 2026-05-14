use crate::db::Repository;
use crate::error::GatewayError;
use crate::models::Route;
use std::sync::Arc;
use uuid::Uuid;

pub struct RouteService {
    repo: Arc<dyn Repository>,
}

impl RouteService {
    pub fn new(repo: Arc<dyn Repository>) -> Self {
        Self { repo }
    }

    pub async fn list(&self) -> Result<Vec<Route>, GatewayError> {
        self.repo.list_routes().await
    }

    pub async fn get(&self, id: &Uuid) -> Result<Option<Route>, GatewayError> {
        self.repo.get_route(id).await
    }

    pub async fn create(&self, route: &Route) -> Result<(), GatewayError> {
        self.repo.create_route(route).await
    }

    pub async fn update(&self, id: &Uuid, route: &Route) -> Result<(), GatewayError> {
        self.repo.update_route(id, route).await
    }

    pub async fn delete(&self, id: &Uuid) -> Result<bool, GatewayError> {
        self.repo.delete_route(id).await
    }
}

impl crate::abstractions::IService for RouteService {
    fn repo(&self) -> &Arc<dyn Repository> {
        &self.repo
    }
}
