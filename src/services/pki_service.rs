use crate::db::Repository;
use crate::error::GatewayError;
use crate::models::Certificate;
use std::sync::Arc;
use uuid::Uuid;

pub struct PkiService {
    repo: Arc<dyn Repository>,
}

impl PkiService {
    pub fn new(repo: Arc<dyn Repository>) -> Self {
        Self { repo }
    }

    pub async fn list_certificates(
        &self,
        service_id: Option<&Uuid>,
    ) -> Result<Vec<Certificate>, GatewayError> {
        self.repo.list_certificates(service_id).await
    }

    pub async fn get(&self, id: &Uuid) -> Result<Option<Certificate>, GatewayError> {
        self.repo.get_certificate(id).await
    }

    pub async fn create(&self, cert: &Certificate) -> Result<(), GatewayError> {
        self.repo.create_certificate(cert).await
    }

    pub async fn update(&self, id: &Uuid, cert: &Certificate) -> Result<(), GatewayError> {
        self.repo.update_certificate(id, cert).await
    }

    pub async fn delete(&self, id: &Uuid) -> Result<bool, GatewayError> {
        self.repo.delete_certificate(id).await
    }
}

impl crate::abstractions::IService for PkiService {
    fn repo(&self) -> &Arc<dyn Repository> {
        &self.repo
    }
}
