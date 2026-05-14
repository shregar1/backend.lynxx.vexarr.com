use serde::Deserialize;

use crate::abstractions::IRequestDto;

#[derive(Debug, Clone, Deserialize)]
pub struct CreateSnapshotBody {
    pub name: String,
}

impl IRequestDto for CreateSnapshotBody {}
