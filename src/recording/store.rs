//! Content-addressed filesystem store for recordings.
//!
//! Layout: `<root>/<sha-prefix-2>/<full-sha>.json`. The two-char prefix
//! shards directories so a single dir doesn't grow past a few thousand
//! entries on disk — fine for ext4/apfs but rough for tooling like `ls`.

use crate::error::GatewayError;
use crate::recording::Recording;
use std::path::PathBuf;

pub struct ContentAddressedStore {
    root: PathBuf,
}

impl ContentAddressedStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &std::path::Path {
        &self.root
    }

    pub async fn put(&self, rec: &Recording) -> Result<(), GatewayError> {
        let id = &rec.id;
        if id.len() < 2 || !id.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(GatewayError::Internal("invalid recording id".into()));
        }
        let prefix = &id[..2];
        let dir = self.root.join(prefix);
        tokio::fs::create_dir_all(&dir)
            .await
            .map_err(|e| GatewayError::Internal(format!("create_dir_all: {e}")))?;
        let path = dir.join(format!("{id}.json"));
        let bytes = serde_json::to_vec_pretty(rec)
            .map_err(|e| GatewayError::Internal(format!("serialize: {e}")))?;
        tokio::fs::write(&path, &bytes)
            .await
            .map_err(|e| GatewayError::Internal(format!("write: {e}")))?;
        Ok(())
    }

    pub async fn get(&self, id: &str) -> Result<Option<Recording>, GatewayError> {
        if id.len() < 2 || !id.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(GatewayError::BadRequest("invalid recording id".into()));
        }
        let path = self.root.join(&id[..2]).join(format!("{id}.json"));
        match tokio::fs::read(&path).await {
            Ok(bytes) => {
                let rec: Recording = serde_json::from_slice(&bytes)
                    .map_err(|e| GatewayError::Internal(format!("parse: {e}")))?;
                Ok(Some(rec))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(GatewayError::Internal(format!("read: {e}"))),
        }
    }

    pub async fn delete(&self, id: &str) -> Result<bool, GatewayError> {
        if id.len() < 2 || !id.chars().all(|c| c.is_ascii_hexdigit()) {
            return Ok(false);
        }
        let path = self.root.join(&id[..2]).join(format!("{id}.json"));
        match tokio::fs::remove_file(&path).await {
            Ok(_) => Ok(true),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(GatewayError::Internal(format!("remove: {e}"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recording::{RecordedRequest, RecordedResponse};
    use chrono::Utc;

    fn sample_recording(id: &str) -> Recording {
        Recording {
            id: id.into(),
            ts: Utc::now(),
            service_id: None,
            route_id: None,
            consumer: None,
            request: RecordedRequest {
                method: "GET".into(),
                path: "/v1/things".into(),
                headers: vec![("accept".into(), "application/json".into())],
                body_b64: String::new(),
            },
            response: RecordedResponse {
                status: 200,
                headers: vec![],
                body_b64: String::new(),
            },
            upstream_url: "http://upstream/v1/things".into(),
            duration_ms: 12,
        }
    }

    #[tokio::test]
    async fn round_trip_put_get() {
        let dir = tempfile::tempdir().unwrap();
        let store = ContentAddressedStore::new(dir.path());
        let rec = sample_recording("abcdef0123456789");
        store.put(&rec).await.unwrap();
        let got = store.get(&rec.id).await.unwrap().expect("present");
        assert_eq!(got.id, rec.id);
        assert_eq!(got.request.path, rec.request.path);
    }

    #[tokio::test]
    async fn get_unknown_id_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        let store = ContentAddressedStore::new(dir.path());
        let got = store.get("deadbeefcafef00d").await.unwrap();
        assert!(got.is_none());
    }

    #[tokio::test]
    async fn bad_id_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let store = ContentAddressedStore::new(dir.path());
        // Non-hex characters are rejected on get.
        assert!(store.get("not-a-hex-id-zz").await.is_err());
        // Too-short id is rejected.
        assert!(store.get("a").await.is_err());
    }

    #[tokio::test]
    async fn delete_returns_true_then_false() {
        let dir = tempfile::tempdir().unwrap();
        let store = ContentAddressedStore::new(dir.path());
        let rec = sample_recording("0011223344556677");
        store.put(&rec).await.unwrap();
        assert!(store.delete(&rec.id).await.unwrap());
        assert!(!store.delete(&rec.id).await.unwrap());
    }
}
