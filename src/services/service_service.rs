use crate::db::Repository;
use crate::error::GatewayError;
use crate::models::Service;
use std::sync::Arc;
use uuid::Uuid;

pub struct ServiceService {
    repo: Arc<dyn Repository>,
}

impl ServiceService {
    pub fn new(repo: Arc<dyn Repository>) -> Self {
        Self { repo }
    }

    pub async fn list(&self) -> Result<Vec<Service>, GatewayError> {
        self.repo.list_services().await
    }

    pub async fn get(&self, id: &Uuid) -> Result<Option<Service>, GatewayError> {
        self.repo.get_service(id).await
    }

    pub async fn get_by_name(&self, name: &str) -> Result<Option<Service>, GatewayError> {
        self.repo.get_service_by_name(name).await
    }

    pub async fn create(&self, svc: &Service) -> Result<(), GatewayError> {
        self.repo.create_service(svc).await
    }

    pub async fn update(&self, id: &Uuid, svc: &Service) -> Result<(), GatewayError> {
        self.repo.update_service(id, svc).await
    }

    pub async fn delete(&self, id: &Uuid) -> Result<bool, GatewayError> {
        self.repo.delete_service(id).await
    }
}

impl crate::abstractions::IService for ServiceService {
    fn repo(&self) -> &Arc<dyn Repository> {
        &self.repo
    }
}
