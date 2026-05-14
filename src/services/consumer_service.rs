use crate::db::Repository;
use crate::error::GatewayError;
use crate::models::Consumer;
use std::sync::Arc;
use uuid::Uuid;

pub struct ConsumerService {
    repo: Arc<dyn Repository>,
}

impl ConsumerService {
    pub fn new(repo: Arc<dyn Repository>) -> Self {
        Self { repo }
    }

    pub async fn list(&self) -> Result<Vec<Consumer>, GatewayError> {
        self.repo.list_consumers().await
    }

    pub async fn create(&self, consumer: &Consumer) -> Result<(), GatewayError> {
        self.repo.create_consumer(consumer).await
    }

    pub async fn delete(&self, id: &Uuid) -> Result<bool, GatewayError> {
        self.repo.delete_consumer(id).await
    }

    pub async fn update(&self, id: &Uuid, consumer: &Consumer) -> Result<(), GatewayError> {
        self.repo.update_consumer(id, consumer).await
    }
}

impl crate::abstractions::IService for ConsumerService {
    fn repo(&self) -> &Arc<dyn Repository> {
        &self.repo
    }
}
