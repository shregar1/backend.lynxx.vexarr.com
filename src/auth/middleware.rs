//! Axum middleware for admin-API authentication and role enforcement.
//!
//! Two layers compose:
//!   * [`extract_token`]: looks up the bearer token, attaches an
//!     `Option<AdminToken>` to request extensions, and 401s on bad tokens.
//!     If the DB has zero tokens we are in *bootstrap mode* and the request
//!     is allowed through without an actor (matches Kong's first-run UX).
//!   * [`require_role`]: per-route guard that 403s if the actor's role is
//!     below the route's minimum.

use crate::admin::routes::AdminState;
use crate::auth::{hash_token, AdminToken, Role};
use axum::extract::State;
use axum::http::{header, Request, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Json;
use chrono::Utc;
use dashmap::DashMap;
use serde_json::json;
use std::sync::Arc;
use std::time::{Duration, Instant};
use uuid::Uuid;

/// In-memory cache for keyx-validated admin tokens. Optional — when
/// the cache is `None` (TTL = 0 in config) every admin request hits
/// keyx. When present, a successful keyx validate populates an entry
/// keyed by the SHA-256 hash of the raw token (raw tokens are never
/// stored in memory); subsequent requests reuse it until `ttl`
/// elapses.
///
/// The TTL is also the upper bound on revocation staleness. Without
/// keyx real-time revocation events, a revoked token keeps working
/// until the entry expires.
pub struct KeyxAuthCache {
    entries: DashMap<String, CachedEntry>,
    ttl: Duration,
}

struct CachedEntry {
    token: AdminToken,
    expires_at: Instant,
}

impl KeyxAuthCache {
    pub fn new(ttl: Duration) -> Self {
        Self {
            entries: DashMap::new(),
            ttl,
        }
    }

    fn get(&self, token_hash: &str) -> Option<AdminToken> {
        let e = self.entries.get(token_hash)?;
        if e.expires_at > Instant::now() {
            Some(e.token.clone())
        } else {
            // Expired entries fall through to a fresh keyx call; we
            // don't bother removing the row here, the next `insert`
            // overwrites it and any other expired entries are pruned
            // opportunistically below.
            None
        }
    }

    fn insert(&self, token_hash: String, token: AdminToken) {
        self.entries.insert(
            token_hash,
            CachedEntry {
                token,
                expires_at: Instant::now() + self.ttl,
            },
        );
    }

    /// Cheap: number of live (potentially-stale) entries. Exposed so
    /// tests can assert cache population.
    pub fn len(&self) -> usize {
        self.entries.len()
    }
}

/// Wrapper around `Option<AdminToken>` so we can `Extension::<CurrentActor>`
/// from handlers without colliding with bare `Option<T>`.
#[derive(Clone, Debug, Default)]
pub struct CurrentActor(pub Option<AdminToken>);

/// Data extracted from a verified PASETO runtime grant.
#[derive(Clone, Debug)]
pub struct GrantActor {
    pub customer_urn: String,
    pub device_id: Uuid,
    pub capabilities: Vec<crate::auth::grant::Capability>,
}

fn unauthorized(msg: &str) -> Response {
    (StatusCode::UNAUTHORIZED, Json(json!({ "error": msg }))).into_response()
}

fn forbidden(msg: &str) -> Response {
    (StatusCode::FORBIDDEN, Json(json!({ "error": msg }))).into_response()
}

/// Middleware that validates the PASETO runtime grant token.
/// Required for all "commercial" endpoints where premium features are executed.
pub async fn validate_grant(
    State(state): State<Arc<crate::auth::handshake::HandshakeState>>,
    mut req: Request<axum::body::Body>,
    next: Next,
) -> Response {
    let auth_header = req.headers().get(header::AUTHORIZATION);
    let token = match auth_header.and_then(|h| h.to_str().ok()).and_then(|s| s.strip_prefix("Bearer ")) {
        Some(t) => t,
        None => return unauthorized("missing runtime grant token"),
    };

    let claims = match state.grant_manager.verify_grant(token) {
        Ok(c) => c,
        Err(e) => return unauthorized(&format!("invalid grant token: {e}")),
    };

    // Check revocation in DB
    match state.repo.get_runtime_grant_by_token_id(&claims.jti).await {
        Ok(Some(grant)) if grant.revoked_at.is_some() => return unauthorized("grant revoked"),
        Ok(Some(_)) => {},
        _ => return unauthorized("grant session not found"),
    };

    let device_id = Uuid::parse_str(&claims.device_id).unwrap_or_else(|_| Uuid::nil());
    
    req.extensions_mut().insert(GrantActor {
        customer_urn: claims.sub,
        device_id,
        capabilities: claims.caps,
    });

    next.run(req).await
}

/// Axum middleware: extract bearer token, look it up, attach a
/// [`CurrentActor`] extension. Bootstrap mode (no tokens in DB) lets the
/// request through with `CurrentActor(None)`.
pub async fn extract_token(
    State(state): State<AdminState>,
    mut req: Request<axum::body::Body>,
    next: Next,
) -> Response {
    let actor = match resolve_actor(&state, req.headers().get(header::AUTHORIZATION)).await {
        Ok(a) => a,
        Err(resp) => return resp,
    };

    // Best-effort `last_used_at` bump — never blocks the request.
    if let Some(token) = actor.as_ref() {
        let repo = state.repo.clone();
        let id = token.id;
        tokio::spawn(async move {
            if let Err(e) = repo.update_admin_token_last_used(&id, Utc::now()).await {
                tracing::warn!(error = %e, token_id = %id, "failed to update last_used_at");
            }
        });
    }

    req.extensions_mut().insert(CurrentActor(actor));
    next.run(req).await
}

async fn resolve_actor(
    state: &AdminState,
    auth_header: Option<&axum::http::HeaderValue>,
) -> Result<Option<AdminToken>, Response> {
    let header_value = match auth_header {
        Some(h) => h,
        None => {
            if !state.auth_required {
                return Ok(None);
            }
            return Err(unauthorized("missing Authorization header"));
        }
    };

    let raw = header_value
        .to_str()
        .ok()
        .and_then(|s| s.strip_prefix("Bearer "))
        .map(str::trim)
        .filter(|s| !s.is_empty());

    let raw = match raw {
        Some(r) => r,
        None => return Err(unauthorized("malformed Authorization header")),
    };

    // Fast path: the Lynxx license API key is always a valid master admin
    // credential. Customers can use the same key they got from their account
    // dashboard at lynxx.vexarr.com — no separate token creation needed.
    if !state.license_api_key.is_empty() && raw == state.license_api_key {
        return Ok(Some(AdminToken {
            id: uuid::Uuid::nil(),
            name: "license-key".to_string(),
            role: crate::auth::Role::Admin,
            created_at: chrono::Utc::now(),
            expires_at: None,
            last_used_at: None,
        }));
    }

    let hash = hash_token(raw);
    let token = state
        .repo
        .get_admin_token_by_hash(&hash)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "admin_tokens lookup failed");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "auth backend error" })),
            )
                .into_response()
        })?;

    let token = match token {
        Some(t) => t,
        None => {
            // Local miss → cache → keyx.
            if let Some(cache) = state.keyx_token_cache.as_ref() {
                if let Some(cached) = cache.get(&hash) {
                    if !cached.is_expired(Utc::now()) {
                        return Ok(Some(cached));
                    }
                }
            }
            if let Some(client) = state.keyx_client.as_ref() {
                let resolved = resolve_via_keyx(client.as_ref(), raw).await?;
                if let Some(tok) = resolved.as_ref() {
                    if let Some(cache) = state.keyx_token_cache.as_ref() {
                        cache.insert(hash.clone(), tok.clone());
                    }
                }
                return Ok(resolved);
            }
            return Err(unauthorized("invalid token"));
        }
    };
    if token.is_expired(Utc::now()) {
        return Err(unauthorized("token expired"));
    }
    Ok(Some(token))
}

/// Synthesise an `AdminToken` from a keyx `/v1/keys/validate` response.
/// Used as the fallback path inside [`resolve_actor`] when the local
/// `admin_tokens` table doesn't know the bearer the caller presented.
async fn resolve_via_keyx(
    client: &crate::integrations::keyx::KeyxClient,
    raw: &str,
) -> Result<Option<AdminToken>, Response> {
    match client.validate_token(raw).await {
        Ok(principal) => match map_keyx_role(&principal.role) {
            Some(role) => Ok(Some(AdminToken {
                id: principal.api_key_urn,
                // First 8 hex chars are enough for log correlation; the
                // full URN goes to the audit log on a separate path.
                name: format!(
                    "keyx:{}:{}",
                    principal.role,
                    &principal.api_key_urn.to_string()[..8]
                ),
                role,
                created_at: Utc::now(),
                expires_at: None,
                last_used_at: None,
            })),
            None => {
                tracing::info!(
                    role = %principal.role,
                    key_urn = %principal.api_key_urn,
                    "keyx role not permitted on lynxx admin surface"
                );
                Err(unauthorized("token role not permitted on lynxx admin"))
            }
        },
        Err(e) => {
            tracing::debug!(error = %e, "keyx validate_token failed");
            Err(unauthorized("invalid token"))
        }
    }
}

/// Map a keyx role string onto a lynxx [`Role`]. Conservative: anything
/// we don't recognise denies access rather than silently widening.
fn map_keyx_role(role: &str) -> Option<Role> {
    match role.to_ascii_lowercase().as_str() {
        "admin" | "owner" => Some(Role::Admin),
        "service" => Some(Role::Operator),
        "pat" | "viewer" => Some(Role::Viewer),
        _ => None,
    }
}

/// Per-role guards. `axum::middleware::from_fn` needs a concrete async fn,
/// so we provide one per role rather than a generic closure. The dispatch
/// shares [`enforce_role`].
pub async fn require_viewer(req: Request<axum::body::Body>, next: Next) -> Response {
    enforce_role(Role::Viewer, req, next).await
}

pub async fn require_operator(req: Request<axum::body::Body>, next: Next) -> Response {
    enforce_role(Role::Operator, req, next).await
}

pub async fn require_admin(req: Request<axum::body::Body>, next: Next) -> Response {
    enforce_role(Role::Admin, req, next).await
}

async fn enforce_role(min: Role, req: Request<axum::body::Body>, next: Next) -> Response {
    let actor = req
        .extensions()
        .get::<CurrentActor>()
        .cloned()
        .unwrap_or_default();
    match actor.0 {
        None => next.run(req).await, // license-key auth or unauthenticated (auth_required=false)
        Some(token) if token.role >= min => next.run(req).await,
        Some(token) => {
            tracing::info!(
                role = %token.role,
                required = %min,
                token_id = %token.id,
                "RBAC denied"
            );
            forbidden(&format!(
                "role '{}' is below required '{}'",
                token.role, min
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::integrations::keyx::{KeyxClient, KeyxConfig};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn keyx_client(uri: String) -> KeyxClient {
        KeyxClient::new(KeyxConfig {
            base_url: uri,
            service_token: "svc".into(),
            license_urn: "00000000-0000-0000-0000-000000000099".into(),
        })
        .unwrap()
    }

    fn envelope(data: serde_json::Value) -> serde_json::Value {
        json!({
            "transactionUrn": "urn:keyx:txn:test",
            "status": "SUCCESS",
            "responseMessage": "OK",
            "responseKey": "SUCCESS",
            "errors": [],
            "timestamp": "2026-05-16T00:00:00+00:00",
            "metadata": {},
            "data": data,
        })
    }

    fn principal(role: &str) -> serde_json::Value {
        json!({
            "tenant_id":   1,
            "tenant_urn":  "00000000-0000-0000-0000-000000000001",
            "api_key_id":  10,
            "api_key_urn": "00000000-0000-0000-0000-0000000000aa",
            "role":        role,
            "key_type":    role,
            "environment": "live",
            "scopes":      [],
        })
    }

    #[test]
    fn role_mapping_is_conservative() {
        assert_eq!(map_keyx_role("admin"), Some(Role::Admin));
        assert_eq!(map_keyx_role("OWNER"), Some(Role::Admin));
        assert_eq!(map_keyx_role("service"), Some(Role::Operator));
        assert_eq!(map_keyx_role("pat"), Some(Role::Viewer));
        assert_eq!(map_keyx_role("viewer"), Some(Role::Viewer));
        assert_eq!(map_keyx_role("publishable"), None);
        assert_eq!(map_keyx_role("restricted"), None);
        assert_eq!(map_keyx_role(""), None);
    }

    #[tokio::test]
    async fn keyx_fallback_returns_admin_for_admin_role() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/keys/validate"))
            .respond_with(ResponseTemplate::new(200).set_body_json(envelope(principal("admin"))))
            .mount(&server)
            .await;
        let client = keyx_client(server.uri());
        let out = resolve_via_keyx(&client, "ak_live_xxx").await.unwrap();
        let tok = out.unwrap();
        assert_eq!(tok.role, Role::Admin);
        assert!(tok.name.starts_with("keyx:admin:"));
    }

    #[tokio::test]
    async fn keyx_fallback_rejects_unknown_role() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(envelope(principal("publishable"))),
            )
            .mount(&server)
            .await;
        let client = keyx_client(server.uri());
        let resp = resolve_via_keyx(&client, "ak_live_xxx").await.unwrap_err();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn keyx_fallback_propagates_keyx_401_as_unauthorized() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(401).set_body_string("nope"))
            .mount(&server)
            .await;
        let client = keyx_client(server.uri());
        let resp = resolve_via_keyx(&client, "bad").await.unwrap_err();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    fn admin_token_stub(role: Role) -> AdminToken {
        AdminToken {
            id: uuid::Uuid::nil(),
            name: "test".into(),
            role,
            created_at: Utc::now(),
            expires_at: None,
            last_used_at: None,
        }
    }

    #[tokio::test]
    async fn cache_miss_returns_none() {
        let cache = KeyxAuthCache::new(Duration::from_secs(60));
        assert!(cache.get("nope").is_none());
        assert_eq!(cache.len(), 0);
    }

    #[tokio::test]
    async fn cache_hit_within_ttl_returns_token() {
        let cache = KeyxAuthCache::new(Duration::from_secs(60));
        cache.insert("h".into(), admin_token_stub(Role::Admin));
        let got = cache.get("h").unwrap();
        assert_eq!(got.role, Role::Admin);
        assert_eq!(cache.len(), 1);
    }

    #[tokio::test]
    async fn cache_get_after_ttl_elapses_returns_none() {
        let cache = KeyxAuthCache::new(Duration::from_millis(20));
        cache.insert("h".into(), admin_token_stub(Role::Operator));
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(cache.get("h").is_none());
    }

    #[tokio::test]
    async fn cache_overwrite_replaces_entry() {
        let cache = KeyxAuthCache::new(Duration::from_secs(60));
        cache.insert("h".into(), admin_token_stub(Role::Viewer));
        cache.insert("h".into(), admin_token_stub(Role::Admin));
        assert_eq!(cache.get("h").unwrap().role, Role::Admin);
        assert_eq!(cache.len(), 1);
    }
}

/// Convenience: pick the right concrete guard for a role. Returns a fn
/// pointer (`Copy`/`Send`/`Sync`/`'static`) so it composes with
/// `axum::middleware::from_fn`.
pub fn require_role(
    min: Role,
) -> fn(
    Request<axum::body::Body>,
    Next,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Response> + Send>> {
    match min {
        Role::Viewer => |req, next| Box::pin(require_viewer(req, next)),
        Role::Operator => |req, next| Box::pin(require_operator(req, next)),
        Role::Admin => |req, next| Box::pin(require_admin(req, next)),
    }
}
