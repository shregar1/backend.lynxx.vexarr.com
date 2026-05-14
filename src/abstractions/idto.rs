use crate::error::GatewayError;

/// Interface for inbound request DTOs. Implement `validate` to enforce
/// business rules before the request reaches the service layer.
pub trait IRequestDto: Send + Sync + serde::de::DeserializeOwned {
    fn validate(&self) -> Result<(), GatewayError> {
        Ok(())
    }
}

/// Marker interface for outbound response DTOs.
pub trait IResponseDto: Send + Sync + serde::Serialize {}
