//! Index-row shape and filter for recordings.
//!
//! This file deliberately does **not** contain SQL. The DB-backed index
//! lives behind the `Repository` trait extension added in session A2;
//! keeping the row + filter types here means the plugin and admin layers
//! can depend on them without pulling in the DB module.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordingIndexRow {
    pub id: String,
    pub ts: DateTime<Utc>,
    pub method: String,
    pub path: String,
    pub status: u16,
    pub duration_ms: u64,
    pub service_id: Option<Uuid>,
    pub route_id: Option<Uuid>,
    pub consumer: Option<String>,
    pub bytes_request: u64,
    pub bytes_response: u64,
}

#[derive(Debug, Clone, Default)]
pub struct RecordingFilter {
    pub since: Option<DateTime<Utc>>,
    pub method: Option<String>,
    pub path_prefix: Option<String>,
    pub status: Option<u16>,
    pub service_id: Option<Uuid>,
    /// 0 → use default of 100. Hard cap of 1000.
    pub limit: usize,
}

impl RecordingFilter {
    pub fn effective_limit(&self) -> usize {
        match self.limit {
            0 => 100,
            n if n > 1000 => 1000,
            n => n,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limit_defaults_and_caps() {
        let f = RecordingFilter::default();
        assert_eq!(f.effective_limit(), 100);

        let f = RecordingFilter {
            limit: 50,
            ..Default::default()
        };
        assert_eq!(f.effective_limit(), 50);

        let f = RecordingFilter {
            limit: 9_999,
            ..Default::default()
        };
        assert_eq!(f.effective_limit(), 1000);
    }
}
