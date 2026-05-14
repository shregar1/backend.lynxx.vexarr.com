use crate::db::Repository;
use crate::error::GatewayError;
use crate::models::PluginInstance;
use std::sync::Arc;
use uuid::Uuid;

pub struct PluginService {
    repo: Arc<dyn Repository>,
}

impl PluginService {
    pub fn new(repo: Arc<dyn Repository>) -> Self {
        Self { repo }
    }

    pub async fn list(&self) -> Result<Vec<PluginInstance>, GatewayError> {
        self.repo.list_plugins().await
    }

    pub async fn get(&self, id: &Uuid) -> Result<Option<PluginInstance>, GatewayError> {
        self.repo.get_plugin(id).await
    }

    pub async fn create(&self, plugin: &PluginInstance) -> Result<(), GatewayError> {
        self.repo.create_plugin(plugin).await
    }

    pub async fn update(&self, id: &Uuid, plugin: &PluginInstance) -> Result<(), GatewayError> {
        self.repo.update_plugin(id, plugin).await
    }

    pub async fn delete(&self, id: &Uuid) -> Result<bool, GatewayError> {
        self.repo.delete_plugin(id).await
    }
}

impl crate::abstractions::IService for PluginService {
    fn repo(&self) -> &Arc<dyn Repository> {
        &self.repo
    }
}
