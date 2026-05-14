use crate::db::Repository;
use crate::error::GatewayError;
use crate::models::ClusterNode;
use std::sync::Arc;
use uuid::Uuid;

pub struct ClusterService {
    repo: Arc<dyn Repository>,
}

impl ClusterService {
    pub fn new(repo: Arc<dyn Repository>) -> Self {
        Self { repo }
    }

    pub async fn list_nodes(&self) -> Result<Vec<ClusterNode>, GatewayError> {
        self.repo.list_cluster_nodes().await
    }

    pub async fn get_node(&self, id: &Uuid) -> Result<Option<ClusterNode>, GatewayError> {
        self.repo.get_cluster_node(id).await
    }

    pub async fn upsert_node(&self, node: &ClusterNode) -> Result<(), GatewayError> {
        self.repo.upsert_cluster_node(node).await
    }

    pub async fn delete_node(&self, id: &Uuid) -> Result<bool, GatewayError> {
        self.repo.delete_cluster_node(id).await
    }
}

impl crate::abstractions::IService for ClusterService {
    fn repo(&self) -> &Arc<dyn Repository> {
        &self.repo
    }
}
