use serde::Deserialize;

use crate::abstractions::IRequestDto;

#[derive(Debug, Clone, Deserialize)]
pub struct CreateConsumer {
    pub username: String,
    pub custom_id: Option<String>,
}

impl IRequestDto for CreateConsumer {}
