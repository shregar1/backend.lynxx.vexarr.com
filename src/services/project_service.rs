use crate::db::Repository;
use crate::error::GatewayError;
use crate::models::{Project, Service};
use std::sync::Arc;
use uuid::Uuid;

pub struct ProjectService {
    repo: Arc<dyn Repository>,
}

impl ProjectService {
    pub fn new(repo: Arc<dyn Repository>) -> Self {
        Self { repo }
    }

    pub async fn list(&self) -> Result<Vec<Project>, GatewayError> {
        self.repo.list_projects().await
    }

    pub async fn get(&self, id: &Uuid) -> Result<Option<Project>, GatewayError> {
        self.repo.get_project(id).await
    }

    pub async fn create(&self, project: &Project) -> Result<(), GatewayError> {
        self.repo.create_project(project).await
    }

    pub async fn update(&self, id: &Uuid, project: &Project) -> Result<(), GatewayError> {
        self.repo.update_project(id, project).await
    }

    pub async fn delete(&self, id: &Uuid) -> Result<bool, GatewayError> {
        self.repo.delete_project(id).await
    }

    pub async fn list_services_for_project(
        &self,
        project_id: &Uuid,
    ) -> Result<Vec<Service>, GatewayError> {
        self.repo.list_services_for_project(project_id).await
    }
}

impl crate::abstractions::IService for ProjectService {
    fn repo(&self) -> &Arc<dyn Repository> {
        &self.repo
    }
}
