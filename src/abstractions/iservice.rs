use crate::db::Repository;
use crate::error::GatewayError;
use std::sync::Arc;

/// Base interface for business-logic services. Each service holds a
/// repository reference and encapsulates domain operations.
pub trait IService: Send + Sync {
    fn repo(&self) -> &Arc<dyn Repository>;
}

/// Convenience result alias used across services.
pub type ServiceResult<T> = Result<T, GatewayError>;
