use crate::db::Repository;
use crate::error::GatewayError;
use crate::models::{SpanRecord, TraceRecord};
use std::sync::Arc;
use uuid::Uuid;

pub struct TracingService {
    repo: Arc<dyn Repository>,
}

impl TracingService {
    pub fn new(repo: Arc<dyn Repository>) -> Self {
        Self { repo }
    }

    pub async fn list_traces(
        &self,
        service_id: &Uuid,
        limit: u32,
    ) -> Result<Vec<TraceRecord>, GatewayError> {
        self.repo.list_traces(service_id, limit).await
    }

    pub async fn get_trace(&self, id: &Uuid) -> Result<Option<TraceRecord>, GatewayError> {
        self.repo.get_trace(id).await
    }

    pub async fn create_trace(&self, trace: &TraceRecord) -> Result<(), GatewayError> {
        self.repo.create_trace(trace).await
    }

    pub async fn list_spans(&self, trace_id: &str) -> Result<Vec<SpanRecord>, GatewayError> {
        self.repo.list_spans(trace_id).await
    }

    pub async fn create_span(&self, span: &SpanRecord) -> Result<(), GatewayError> {
        self.repo.create_span(span).await
    }
}

impl crate::abstractions::IService for TracingService {
    fn repo(&self) -> &Arc<dyn Repository> {
        &self.repo
    }
}
