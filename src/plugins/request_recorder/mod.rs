//! Time-Travel Debug capture plugin (`request-recorder`).
//!
//! On the request side we sample, buffer the body up to `max_body_bytes`,
//! redact configured headers and JSON body keys, and stash the captured
//! state in a process-local map keyed by a uuid header. On the response
//! side we look that state back up, capture the response body (already
//! buffered into `Full<Bytes>` by the proxy), build a `Recording`, and
//! fire-and-forget it onto a bounded mpsc. A background task writes to
//! the content-addressed store and the DB index. A second task sweeps
//! retention hourly.
//!
//! Failures never fail the request — the gateway continues even if the
//! disk or DB is unavailable.

mod config;

use crate::db::Repository;
use crate::error::{GatewayError, PluginError};
use crate::plugins::{BoxBody, Plugin};
use crate::recording::{
    redact, ContentAddressedStore, RecordedRequest, RecordedResponse, Recording, RecordingIndexRow,
};
use async_trait::async_trait;
use axum::http::{HeaderValue, Request, Response};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use chrono::Utc;
use config::Config;
use dashmap::DashMap;
use http_body_util::BodyExt;
use std::sync::{Arc, OnceLock};
use std::time::Instant;
use axum::body::Body;

/// Header used to thread the pending capture id from request to response.
const HDR_CAPTURE_ID: &str = "x-lynxx-recorder-id";

/// Process-global pending-capture map. Plugins are rebuilt on every admin
/// mutation, so a per-instance map would lose in-flight captures across
/// rebuilds. The map is keyed by a uuid the request side mints.
static PENDING: OnceLock<Arc<DashMap<String, PendingCapture>>> = OnceLock::new();

fn pending() -> &'static Arc<DashMap<String, PendingCapture>> {
    PENDING.get_or_init(|| Arc::new(DashMap::new()))
}

#[derive(Clone)]
struct PendingCapture {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
    started: Instant,
    started_utc: chrono::DateTime<Utc>,
    cfg_min_status: u16,
    cfg_max_status: u16,
    cfg_max_body_bytes: usize,
    /// Match context fields plumbed in by the proxy handler. `None` if the
    /// request didn't go through the proxy match path (e.g. unit tests).
    service_id: Option<uuid::Uuid>,
    route_id: Option<uuid::Uuid>,
    upstream_url: String,
    consumer: Option<String>,
}

struct RecordingJob {
    recording: Recording,
    index_row: RecordingIndexRow,
}

pub struct RequestRecorderPlugin {
    cfg: Config,
    tx: tokio::sync::mpsc::Sender<RecordingJob>,
}

impl RequestRecorderPlugin {
    pub fn from_config(
        cfg_value: &serde_json::Value,
        repo: Option<Arc<dyn Repository>>,
    ) -> Result<Self, GatewayError> {
        let cfg: Config = if cfg_value.is_null() {
            Config::default()
        } else {
            serde_json::from_value(cfg_value.clone())
                .map_err(|e| GatewayError::BadRequest(format!("request-recorder: {e}")))?
        };
        let repo = repo.ok_or_else(|| {
            GatewayError::BadRequest("request-recorder requires repository".into())
        })?;
        let store = Arc::new(ContentAddressedStore::new(&cfg.directory));
        let (tx, rx) = tokio::sync::mpsc::channel::<RecordingJob>(1024);
        spawn_writer(rx, store, repo.clone());
        spawn_retention_sweeper(repo, cfg.retention_days);
        Ok(Self { cfg, tx })
    }
}

#[async_trait]
impl Plugin for RequestRecorderPlugin {
    fn name(&self) -> &str {
        "request-recorder"
    }

    async fn on_request(&self, req: &mut Request<Body>) -> Result<(), PluginError> {
        // Sampling decision is made up front; if we're not capturing this
        // request we don't touch the body at all.
        if rand::random::<f64>() >= self.cfg.sample_rate {
            return Ok(());
        }

        // Pull match context out of extensions before we rip the request
        // apart. The proxy inserts this just before the plugin chain runs;
        // for non-proxy paths (e.g. unit tests) it'll just be absent.
        let match_ctx = req
            .extensions()
            .get::<crate::proxy::handler::MatchContext>()
            .cloned();

        // Drain the request body so we can both record it and rebuild the
        // request for downstream consumers.
        let (parts, body) =
            std::mem::replace(req, Request::new(Body::empty())).into_parts();
        let body_bytes = match body.collect().await {
            Ok(c) => c.to_bytes(),
            Err(e) => {
                tracing::debug!(error = %e, "request-recorder: collect req body");
                // Reassemble with empty body — we lost the original. Better
                // than panicking the request.
                *req = Request::from_parts(parts, Body::empty());
                return Ok(());
            }
        };
        let truncated: Vec<u8> = if body_bytes.len() > self.cfg.max_body_bytes {
            body_bytes[..self.cfg.max_body_bytes].to_vec()
        } else {
            body_bytes.to_vec()
        };

        let mut headers: Vec<(String, String)> = parts
            .headers
            .iter()
            .map(|(k, v)| (k.as_str().to_string(), v.to_str().unwrap_or("").to_string()))
            .collect();
        redact::redact_headers(&mut headers, &self.cfg.redact_headers);

        // Body redaction only applies to JSON; non-JSON bodies are recorded as-is.
        let body_for_record: Vec<u8> = match serde_json::from_slice::<serde_json::Value>(&truncated)
        {
            Ok(mut v) => {
                redact::redact_body_paths(&mut v, &self.cfg.redact_body_paths);
                serde_json::to_vec(&v).unwrap_or_else(|_| truncated.clone())
            }
            Err(_) => truncated.clone(),
        };

        // Prefer the consumer the proxy already resolved (from the
        // X-Consumer-Username request header at match time); fall back to
        // whatever the auth plugins wrote on the way through.
        let consumer_from_header = parts
            .headers
            .get("x-consumer-username")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());
        let (mc_service, mc_route, mc_upstream, mc_consumer) = match_ctx.map(|c| (
            Some(c.service_id),
            Some(c.route_id),
            c.upstream_url,
            c.consumer_username.or(consumer_from_header.clone()),
        )).unwrap_or((None, None, String::new(), consumer_from_header));

        let pending_capture = PendingCapture {
            method: parts.method.as_str().to_string(),
            path: parts.uri.path().to_string(),
            headers,
            body: body_for_record,
            started: Instant::now(),
            started_utc: Utc::now(),
            cfg_min_status: self.cfg.min_status_to_record,
            cfg_max_status: self.cfg.max_status_to_record,
            cfg_max_body_bytes: self.cfg.max_body_bytes,
            service_id: mc_service,
            route_id: mc_route,
            upstream_url: mc_upstream,
            consumer: mc_consumer,
        };

        // Mint an id, stash the capture, and thread the id forward via a
        // header. The proxy handler copies request headers onto the response
        // before running the response chain, so this id reaches `on_response`.
        let capture_id = uuid::Uuid::new_v4().to_string();
        pending().insert(capture_id.clone(), pending_capture);

        // Reassemble request with the buffered body so downstream consumers
        // still see it.
        let mut req_rebuilt = Request::from_parts(parts, Body::from(body_bytes));
        if let Ok(v) = HeaderValue::from_str(&capture_id) {
            req_rebuilt.headers_mut().insert(HDR_CAPTURE_ID, v);
        }
        *req = req_rebuilt;
        Ok(())
    }

    async fn on_response(&self, resp: &mut Response<BoxBody>) -> Result<(), PluginError> {
        let capture_id = match resp
            .headers()
            .get(HDR_CAPTURE_ID)
            .and_then(|v| v.to_str().ok())
            .map(String::from)
        {
            Some(k) => k,
            None => return Ok(()),
        };
        // Internal header — don't leak it downstream.
        resp.headers_mut().remove(HDR_CAPTURE_ID);

        let pending_capture = match pending().remove(&capture_id) {
            Some((_, p)) => p,
            None => return Ok(()),
        };

        let status = resp.status().as_u16();
        if status < pending_capture.cfg_min_status || status > pending_capture.cfg_max_status {
            return Ok(());
        }

        // Drain the response body so we can both record it and rebuild the
        // response for downstream consumers.
        let (parts, body) =
            std::mem::replace(resp, Response::new(Body::empty())).into_parts();
        let body_bytes = match body.collect().await {
            Ok(c) => c.to_bytes(),
            Err(e) => {
                tracing::debug!(error = %e, "request-recorder: collect resp body");
                *resp = Response::from_parts(parts, Body::empty());
                return Ok(());
            }
        };
        let truncated: Vec<u8> = if body_bytes.len() > pending_capture.cfg_max_body_bytes {
            body_bytes[..pending_capture.cfg_max_body_bytes].to_vec()
        } else {
            body_bytes.to_vec()
        };

        let resp_headers: Vec<(String, String)> = parts
            .headers
            .iter()
            .map(|(k, v)| (k.as_str().to_string(), v.to_str().unwrap_or("").to_string()))
            .collect();

        let duration_ms = pending_capture.started.elapsed().as_millis() as u64;

        // Build a Recording. compute_id requires id="" first so the round-trip
        // file content matches the addressed name.
        let mut recording = Recording {
            id: String::new(),
            ts: pending_capture.started_utc,
            service_id: pending_capture.service_id,
            route_id: pending_capture.route_id,
            consumer: pending_capture.consumer.clone(),
            request: RecordedRequest {
                method: pending_capture.method.clone(),
                path: pending_capture.path.clone(),
                headers: pending_capture.headers.clone(),
                body_b64: STANDARD.encode(&pending_capture.body),
            },
            response: RecordedResponse {
                status,
                headers: resp_headers,
                body_b64: STANDARD.encode(&truncated),
            },
            upstream_url: pending_capture.upstream_url.clone(),
            duration_ms,
        };
        let id = recording.compute_id();
        recording.id = id.clone();

        let index_row = RecordingIndexRow {
            id: id.clone(),
            ts: pending_capture.started_utc,
            method: pending_capture.method,
            path: pending_capture.path,
            status,
            duration_ms,
            service_id: pending_capture.service_id,
            route_id: pending_capture.route_id,
            consumer: pending_capture.consumer,
            bytes_request: pending_capture.body.len() as u64,
            bytes_response: truncated.len() as u64,
        };

        // Reassemble response with the buffered body.
        *resp = Response::from_parts(parts, Body::from(body_bytes));

        // Fire-and-forget. A full channel just drops; recording is best-effort.
        if let Err(e) = self.tx.try_send(RecordingJob {
            recording,
            index_row,
        }) {
            tracing::debug!(error = %e, "request-recorder: drop (channel full)");
        }
        Ok(())
    }
}

fn spawn_writer(
    mut rx: tokio::sync::mpsc::Receiver<RecordingJob>,
    store: Arc<ContentAddressedStore>,
    repo: Arc<dyn Repository>,
) {
    tokio::spawn(async move {
        while let Some(job) = rx.recv().await {
            if let Err(e) = store.put(&job.recording).await {
                tracing::warn!(error = %e, "request-recorder: store.put failed");
                continue;
            }
            if let Err(e) = repo.append_recording(&job.index_row).await {
                tracing::warn!(error = %e, "request-recorder: append_recording failed");
            }
        }
    });
}

fn spawn_retention_sweeper(repo: Arc<dyn Repository>, retention_days: u32) {
    if retention_days == 0 {
        return;
    }
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(3600));
        interval.tick().await; // skip immediate first tick
        loop {
            interval.tick().await;
            let cutoff = Utc::now() - chrono::Duration::days(retention_days as i64);
            match repo.delete_recordings_before(cutoff).await {
                Ok(n) if n > 0 => {
                    tracing::info!(deleted = n, "request-recorder retention sweep")
                }
                Ok(_) => {}
                Err(e) => tracing::warn!(error = %e, "request-recorder retention sweep failed"),
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    struct StubRepo;

    #[async_trait]
    impl Repository for StubRepo {
        async fn list_services(&self) -> Result<Vec<crate::models::Service>, GatewayError> { Ok(vec![]) }
        async fn get_service(&self, _id: &uuid::Uuid) -> Result<Option<crate::models::Service>, GatewayError> { Ok(None) }
        async fn create_service(&self, _svc: &crate::models::Service) -> Result<(), GatewayError> { Ok(()) }
        async fn update_service(&self, _id: &uuid::Uuid, _svc: &crate::models::Service) -> Result<(), GatewayError> { Ok(()) }
        async fn delete_service(&self, _id: &uuid::Uuid) -> Result<bool, GatewayError> { Ok(false) }
        async fn list_routes(&self) -> Result<Vec<crate::models::Route>, GatewayError> { Ok(vec![]) }
        async fn get_route(&self, _id: &uuid::Uuid) -> Result<Option<crate::models::Route>, GatewayError> { Ok(None) }
        async fn create_route(&self, _route: &crate::models::Route) -> Result<(), GatewayError> { Ok(()) }
        async fn update_route(&self, _id: &uuid::Uuid, _route: &crate::models::Route) -> Result<(), GatewayError> { Ok(()) }
        async fn delete_route(&self, _id: &uuid::Uuid) -> Result<bool, GatewayError> { Ok(false) }
        async fn list_plugins(&self) -> Result<Vec<crate::models::PluginInstance>, GatewayError> { Ok(vec![]) }
        async fn get_plugin(&self, _id: &uuid::Uuid) -> Result<Option<crate::models::PluginInstance>, GatewayError> { Ok(None) }
        async fn create_plugin(&self, _plugin: &crate::models::PluginInstance) -> Result<(), GatewayError> { Ok(()) }
        async fn update_plugin(&self, _id: &uuid::Uuid, _plugin: &crate::models::PluginInstance) -> Result<(), GatewayError> { Ok(()) }
        async fn delete_plugin(&self, _id: &uuid::Uuid) -> Result<bool, GatewayError> { Ok(false) }
        async fn list_consumers(&self) -> Result<Vec<crate::models::Consumer>, GatewayError> { Ok(vec![]) }
        async fn create_consumer(&self, _consumer: &crate::models::Consumer) -> Result<(), GatewayError> { Ok(()) }
        async fn delete_consumer(&self, _id: &uuid::Uuid) -> Result<bool, GatewayError> { Ok(false) }
        async fn list_targets(&self) -> Result<Vec<crate::models::Target>, GatewayError> { Ok(vec![]) }
        async fn list_targets_for_service(&self, _id: &uuid::Uuid) -> Result<Vec<crate::models::Target>, GatewayError> { Ok(vec![]) }
        async fn get_target(&self, _id: &uuid::Uuid) -> Result<Option<crate::models::Target>, GatewayError> { Ok(None) }
        async fn create_target(&self, _t: &crate::models::Target) -> Result<(), GatewayError> { Ok(()) }
        async fn update_target(&self, _id: &uuid::Uuid, _t: &crate::models::Target) -> Result<(), GatewayError> { Ok(()) }
        async fn delete_target(&self, _id: &uuid::Uuid) -> Result<bool, GatewayError> { Ok(false) }
        async fn is_empty(&self) -> Result<bool, GatewayError> { Ok(true) }
        async fn seed_from_config(&self, _config: &crate::config::GatewayConfig) -> Result<(), GatewayError> { Ok(()) }
        async fn list_admin_tokens(&self) -> Result<Vec<crate::auth::AdminToken>, GatewayError> { Ok(vec![]) }
        async fn get_admin_token_by_hash(&self, _h: &str) -> Result<Option<crate::auth::AdminToken>, GatewayError> { Ok(None) }
        async fn get_admin_token(&self, _id: &uuid::Uuid) -> Result<Option<crate::auth::AdminToken>, GatewayError> { Ok(None) }
        async fn create_admin_token(&self, _t: &crate::db::NewAdminToken) -> Result<(), GatewayError> { Ok(()) }
        async fn update_admin_token(&self, _id: &uuid::Uuid, _patch: &crate::db::AdminTokenPatch) -> Result<Option<crate::auth::AdminToken>, GatewayError> { Ok(None) }
        async fn delete_admin_token(&self, _id: &uuid::Uuid) -> Result<bool, GatewayError> { Ok(false) }
        async fn update_admin_token_last_used(&self, _id: &uuid::Uuid, _ts: chrono::DateTime<Utc>) -> Result<(), GatewayError> { Ok(()) }
        async fn has_any_admin_token(&self) -> Result<bool, GatewayError> { Ok(false) }
        async fn append_audit(&self, entry: &crate::audit::NewAuditEntry) -> Result<crate::audit::AuditEntry, GatewayError> {
            Ok(crate::audit::AuditEntry {
                id: 0, ts: entry.ts, actor_id: entry.actor_id, actor_name: entry.actor_name.clone(), actor_role: entry.actor_role.clone(),
                action: entry.action.clone(), resource_type: entry.resource_type.clone(), resource_id: entry.resource_id.clone(),
                diff: entry.diff.clone(), hash: String::new(), prev_hash: None,
            })
        }
        async fn list_audit(&self, _f: &crate::audit::AuditFilter) -> Result<Vec<crate::audit::AuditEntry>, GatewayError> { Ok(vec![]) }
        async fn last_audit_hash(&self) -> Result<Option<String>, GatewayError> { Ok(None) }
        fn cluster_bus(&self) -> Arc<dyn crate::cluster::ClusterBus> { unimplemented!("test stub") }
        async fn get_quota_usage(&self, _period: &str) -> Result<Option<(u64, u64)>, GatewayError> { Ok(None) }
        async fn upsert_quota_usage(&self, _period: &str, _calls_total: u64, _calls_rejected: u64) -> Result<(), GatewayError> { Ok(()) }
        async fn append_recording(&self, _row: &RecordingIndexRow) -> Result<(), GatewayError> { Ok(()) }
        async fn list_recordings(&self, _f: &crate::recording::RecordingFilter) -> Result<Vec<RecordingIndexRow>, GatewayError> { Ok(vec![]) }
        async fn get_recording_index(&self, _id: &str) -> Result<Option<RecordingIndexRow>, GatewayError> { Ok(None) }
        async fn delete_recording(&self, _id: &str) -> Result<bool, GatewayError> { Ok(false) }
        async fn delete_recordings_before(&self, _ts: chrono::DateTime<chrono::Utc>) -> Result<u64, GatewayError> { Ok(0) }
        async fn get_device_fingerprint(&self, _customer_id: i64, _hardware_id: &str) -> Result<Option<crate::models::DeviceFingerprint>, GatewayError> { Ok(None) }
        async fn create_device_fingerprint(&self, _device: &crate::models::DeviceFingerprint) -> Result<(), GatewayError> { Ok(()) }
        async fn update_device_last_seen(&self, _id: &uuid::Uuid, _ts: chrono::DateTime<chrono::Utc>) -> Result<(), GatewayError> { Ok(()) }
        async fn create_runtime_grant(&self, _grant: &crate::models::RuntimeGrant) -> Result<(), GatewayError> { Ok(()) }
        async fn get_runtime_grant_by_token_id(&self, _token_id: &str) -> Result<Option<crate::models::RuntimeGrant>, GatewayError> { Ok(None) }
        async fn update_grant_heartbeat(&self, _token_id: &str, _ts: chrono::DateTime<chrono::Utc>) -> Result<(), GatewayError> { Ok(()) }
        async fn revoke_grant(&self, _token_id: &str) -> Result<bool, GatewayError> { Ok(false) }
        async fn get_api_key_by_prefix(&self, _prefix: &str) -> Result<Option<crate::models::ApiKeyRow>, GatewayError> { Ok(None) }
    }

    #[tokio::test]
    async fn from_config_default_builds_ok() {
        let repo: Arc<dyn Repository> = Arc::new(StubRepo);
        let v = serde_json::json!({});
        let p = RequestRecorderPlugin::from_config(&v, Some(repo)).unwrap();
        assert_eq!(p.name(), "request-recorder");
    }

    #[tokio::test]
    async fn from_config_out_of_range_sample_rate_still_builds() {
        let repo: Arc<dyn Repository> = Arc::new(StubRepo);
        let v = serde_json::json!({"sample_rate": 99.0, "retention_days": 0});
        let p = RequestRecorderPlugin::from_config(&v, Some(repo)).unwrap();
        assert_eq!(p.name(), "request-recorder");
    }

    #[tokio::test]
    async fn from_config_without_repo_errors() {
        let v = serde_json::json!({});
        let res = RequestRecorderPlugin::from_config(&v, None);
        assert!(res.is_err());
    }
}
