//! Time-Travel Debug — recording primitives.
//!
//! This module is self-contained: it defines the in-memory shape of a
//! captured request/response pair, a content-addressed filesystem store
//! for those recordings, header/JSON-path redaction helpers, and a
//! structural diff used by the replayer.
//!
//! The plugin that actually captures traffic, the DB-backed index, and
//! the admin/CLI surface live in later sessions (A2 + A3). Nothing in
//! this module touches the running gateway.

pub mod index;
pub mod redact;
pub mod replayer;
pub mod store;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub use index::{RecordingFilter, RecordingIndexRow};
pub use replayer::{replay, ReplayResult};
pub use store::ContentAddressedStore;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Recording {
    pub id: String,
    pub ts: DateTime<Utc>,
    pub service_id: Option<Uuid>,
    pub route_id: Option<Uuid>,
    pub consumer: Option<String>,
    pub request: RecordedRequest,
    pub response: RecordedResponse,
    pub upstream_url: String,
    pub duration_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordedRequest {
    pub method: String,
    pub path: String,
    pub headers: Vec<(String, String)>,
    pub body_b64: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordedResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body_b64: String,
}

impl Recording {
    /// Compute the content-addressed id (sha256 of canonical JSON bytes).
    ///
    /// The `id` field is included in the hash input by design: callers
    /// should set `id` to the empty string before computing, then assign
    /// the returned hash. This keeps the round-trip on-disk representation
    /// stable — the file's id matches the id stored inside it.
    pub fn compute_id(&self) -> String {
        use sha2::{Digest, Sha256};
        let bytes = serde_json::to_vec(self).expect("recording serializes");
        let mut h = Sha256::new();
        h.update(&bytes);
        format!("{:x}", h.finalize())
    }
}
