use serde::Deserialize;

use crate::abstractions::IRequestDto;

#[derive(Debug, Clone, Deserialize)]
pub struct PatchCircuitBreakerBody {
    pub failure_threshold: Option<u32>,
    pub success_threshold: Option<u32>,
    pub open_duration_ms: Option<u64>,
    /// Optionally force-reset the circuit to closed.
    #[serde(default)]
    pub reset: bool,
}

impl IRequestDto for PatchCircuitBreakerBody {}
