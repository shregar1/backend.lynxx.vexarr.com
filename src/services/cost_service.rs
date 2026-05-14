use crate::db::Repository;
use crate::error::GatewayError;
use crate::models::CostRecord;
use chrono::{DateTime, Utc};
use std::sync::Arc;
use uuid::Uuid;

pub struct CostService {
    repo: Arc<dyn Repository>,
}

impl CostService {
    pub fn new(repo: Arc<dyn Repository>) -> Self {
        Self { repo }
    }

    pub async fn list(
        &self,
        service_id: &Uuid,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
    ) -> Result<Vec<CostRecord>, GatewayError> {
        self.repo.list_cost_records(service_id, from, to).await
    }

    pub async fn upsert(&self, record: &CostRecord) -> Result<(), GatewayError> {
        self.repo.upsert_cost_record(record).await
    }
}

impl crate::abstractions::IService for CostService {
    fn repo(&self) -> &Arc<dyn Repository> {
        &self.repo
    }
}
