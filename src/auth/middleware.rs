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
use serde_json::json;
use std::sync::Arc;
use uuid::Uuid;

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
    // Bootstrap: if there are zero tokens, allow everything (warn at
    // startup is logged in main.rs). The moment any token exists, auth is
    // mandatory regardless of the auth_required config flag.
    let has_any = match state.repo.has_any_admin_token().await {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(error = %e, "failed to query admin_tokens for bootstrap check");
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "auth backend unavailable" })),
            )
                .into_response());
        }
    };

    let header_value = match auth_header {
        Some(h) => h,
        None => {
            if !has_any && !state.auth_required {
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

    let token = token.ok_or_else(|| unauthorized("invalid token"))?;
    if token.is_expired(Utc::now()) {
        return Err(unauthorized("token expired"));
    }
    Ok(Some(token))
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
        None => next.run(req).await, // bootstrap mode
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
