//! Axum router for the billing service.

use axum::{
    body::Bytes,
    extract::{Path, Query, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use tower_http::cors::{Any, CorsLayer};
use tracing::{error, info, warn};
use uuid::Uuid;

use super::auth::{hash_password, issue_session, verify_password, verify_session};
use super::keys::{generate as gen_key, hash_key};
use super::models::{Customer, NewApiKey, NewCustomer};
use super::stripe;
use super::AppState;

pub fn router(state: AppState) -> Router {
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    Router::new()
        .route("/v1/license/verify", get(verify_license))
        .route("/v1/auth/signup", post(signup))
        .route("/v1/auth/signin", post(signin))
        .route("/v1/account/me", get(me))
        .route("/v1/account/keys", get(list_keys).post(create_key))
        .route("/v1/account/keys/:urn/revoke", post(revoke_key))
        .route("/v1/billing/checkout", post(start_checkout))
        .route("/v1/billing/portal", get(billing_portal))
        .route("/v1/stripe/webhook", post(stripe_webhook))
        // Admin-only — intentionally NOT exposed via the website. The
        // public /v1/auth/signup endpoint rejects subscription_type=internal,
        // so the only path to provision an internal dev account is via
        // this endpoint with the master admin secret.
        .route("/v1/admin/customers/internal", post(admin_create_internal))
        .route("/install.sh", get(install_script))
        .route("/install", get(install_script))
        // Returns a short-lived V4 signed URL for the requested binary.
        // The install.sh script calls this once per binary it needs.
        .route("/v1/install/binary", get(install_binary))
        // /healthz on *.run.app is intercepted by Google's edge (returns
        // a generic 404 HTML page before the request reaches the
        // container), so we expose /health as the public probe.
        .route("/health", get(|| async { "ok" }))
        .route("/healthz", get(|| async { "ok" }))
        .layer(cors)
        .with_state(state)
}

// install.sh is baked into the binary via include_str! so the billing
// service can serve it without filesystem access at runtime. Update the
// script in scripts/install.sh and rebuild to roll a new version.
const INSTALL_SCRIPT: &str = include_str!("../../scripts/install.sh");

async fn install_script() -> Response {
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "text/x-shellscript; charset=utf-8"),
            (header::CACHE_CONTROL, "public, max-age=300"),
        ],
        INSTALL_SCRIPT,
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// Install: signed URL for binary downloads
// ---------------------------------------------------------------------------

/// Allowlist of (binary, os, arch) triples we serve. Anything outside
/// this list returns 404 — prevents the endpoint from being abused as
/// a generic GCS signer for arbitrary objects in the bucket.
const ALLOWED_BINARIES: &[&str] = &["lynxx", "lynx-gateway"];
const ALLOWED_OS: &[&str] = &["linux", "darwin"];
const ALLOWED_ARCH: &[&str] = &["amd64", "arm64"];
const SIGNED_URL_TTL_SECS: i64 = 300; // 5 minutes

#[derive(Deserialize)]
struct InstallBinaryQuery {
    name: String,
    os: String,
    arch: String,
}

#[derive(Serialize)]
struct InstallBinaryResponse {
    url: String,
    expires_at: i64,
}

async fn install_binary(
    State(state): State<AppState>,
    Query(q): Query<InstallBinaryQuery>,
) -> Result<Json<InstallBinaryResponse>, ApiError> {
    if !ALLOWED_BINARIES.contains(&q.name.as_str()) {
        return Err(ApiError(StatusCode::NOT_FOUND, "unknown binary".into()));
    }
    if !ALLOWED_OS.contains(&q.os.as_str()) {
        return Err(ApiError(StatusCode::BAD_REQUEST, "unsupported os".into()));
    }
    if !ALLOWED_ARCH.contains(&q.arch.as_str()) {
        return Err(ApiError(StatusCode::BAD_REQUEST, "unsupported arch".into()));
    }
    let signer = state.gcs.as_ref().ok_or_else(|| {
        ApiError(
            StatusCode::SERVICE_UNAVAILABLE,
            "binary distribution disabled (no GCS bucket configured)".into(),
        )
    })?;
    let object = format!(
        "{os}/{arch}/{name}",
        os = q.os,
        arch = q.arch,
        name = q.name
    );
    let url = signer
        .sign_get(&object, SIGNED_URL_TTL_SECS)
        .await
        .map_err(|e| {
            error!(error=%e, "sign GCS URL failed");
            ApiError(
                StatusCode::BAD_GATEWAY,
                "could not sign download URL".into(),
            )
        })?;
    Ok(Json(InstallBinaryResponse {
        url,
        expires_at: Utc::now().timestamp() + SIGNED_URL_TTL_SECS,
    }))
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

struct ApiError(StatusCode, String);

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(json!({ "error": self.1 }))).into_response()
    }
}

impl From<anyhow::Error> for ApiError {
    fn from(e: anyhow::Error) -> Self {
        error!(error = %e, "billing internal error");
        ApiError(StatusCode::INTERNAL_SERVER_ERROR, "internal error".into())
    }
}

fn bad(msg: impl Into<String>) -> ApiError {
    ApiError(StatusCode::BAD_REQUEST, msg.into())
}

fn unauthorized() -> ApiError {
    ApiError(StatusCode::UNAUTHORIZED, "unauthorized".into())
}

// ---------------------------------------------------------------------------
// Auth helpers
// ---------------------------------------------------------------------------

async fn require_session(state: &AppState, headers: &HeaderMap) -> Result<Customer, ApiError> {
    let token = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .ok_or_else(unauthorized)?;
    let customer_id = verify_session(token, &state.session_secret).ok_or_else(unauthorized)?;
    state
        .repo
        .find_customer(customer_id)
        .await?
        .ok_or_else(unauthorized)
}

/// Constant-time bearer-token check against the master admin secret.
/// Used to gate `/v1/admin/*` endpoints which provision internal-dev
/// customers and other powers that the public surface must never expose.
fn require_admin(state: &AppState, headers: &HeaderMap) -> Result<(), ApiError> {
    let token = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .ok_or_else(unauthorized)?;
    let a = token.as_bytes();
    let b = state.admin_api_key.as_bytes();
    if a.len() != b.len() {
        return Err(unauthorized());
    }
    let mut diff: u8 = 0;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    if diff == 0 {
        Ok(())
    } else {
        Err(unauthorized())
    }
}

// ---------------------------------------------------------------------------
// Public: licence verification (called by the gateway on startup)
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct VerifyQuery {
    key: String,
}

#[derive(Serialize)]
struct VerifyResponse {
    customer_urn: String,
    tier: String,
    scopes: Vec<String>,
    expires_at: Option<i64>,
}

async fn verify_license(
    State(state): State<AppState>,
    Query(q): Query<VerifyQuery>,
) -> Result<Json<VerifyResponse>, ApiError> {
    let hash = hash_key(&q.key);
    let key = state
        .repo
        .find_api_key_by_hash(&hash)
        .await?
        .ok_or(ApiError(StatusCode::UNAUTHORIZED, "invalid key".into()))?;

    if key.status_code != "active" {
        return Err(ApiError(
            StatusCode::FORBIDDEN,
            format!("key {}", key.status_code),
        ));
    }
    if let Some(exp) = key.expires_at {
        if exp <= Utc::now() {
            return Err(ApiError(StatusCode::FORBIDDEN, "key expired".into()));
        }
    }

    let customer = state
        .repo
        .find_customer(key.customer_id)
        .await?
        .ok_or(ApiError(
            StatusCode::UNAUTHORIZED,
            "customer missing".into(),
        ))?;
    if customer.subscription_status_code != "active" {
        return Err(ApiError(
            StatusCode::FORBIDDEN,
            format!("subscription {}", customer.subscription_status_code),
        ));
    }

    // Side effect: touch last_used_at (fire-and-forget; failure is not fatal).
    let repo = state.repo.clone();
    let kid = key.id;
    tokio::spawn(async move {
        let _ = repo.touch_last_used(kid).await;
    });

    Ok(Json(VerifyResponse {
        customer_urn: customer.urn.to_string(),
        tier: customer.tier_code,
        scopes: key.scopes.clone(),
        expires_at: key.expires_at.map(|t| t.timestamp()),
    }))
}

// ---------------------------------------------------------------------------
// Signup
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct SignupReq {
    email: String,
    password: String,
    name: Option<String>,
    company: Option<String>,
    #[serde(default = "default_tier")]
    tier: String,
}

fn default_tier() -> String {
    "free".to_string()
}

#[derive(Serialize)]
struct SignupResp {
    customer_urn: String,
    session_token: String,
    /// Present only when `tier == "free"` — the user's first API key.
    api_key: Option<String>,
    /// Present only when a paid tier was requested AND Stripe is configured.
    checkout_url: Option<String>,
}

async fn signup(
    State(state): State<AppState>,
    Json(req): Json<SignupReq>,
) -> Result<Json<SignupResp>, ApiError> {
    let email = req.email.trim().to_lowercase();
    if !email.contains('@') {
        return Err(bad("invalid email"));
    }
    if req.password.len() < 8 {
        return Err(bad("password must be at least 8 characters"));
    }
    let tier = req.tier.to_lowercase();
    if !matches!(tier.as_str(), "free" | "pro" | "business") {
        return Err(bad("tier must be free, pro, or business"));
    }
    if state.repo.find_customer_by_email(&email).await?.is_some() {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "email already registered".into(),
        ));
    }

    let hash = hash_password(&req.password)?;
    let urn = Uuid::new_v4();
    let new_customer = NewCustomer {
        urn,
        email: email.clone(),
        password_hash: hash,
        name: req.name.filter(|s| !s.is_empty()),
        company: req.company.filter(|s| !s.is_empty()),
        tier_code: "free".to_string(),
        stripe_customer_id: None,
        stripe_subscription_id: None,
        subscription_status_code: "active".into(),
        subscription_type_code: "free".into(),
    };
    let customer_id = state.repo.create_customer(&new_customer).await?;
    let session_token = issue_session(customer_id, &state.session_secret);

    let mut resp = SignupResp {
        customer_urn: urn.to_string(),
        session_token,
        api_key: None,
        checkout_url: None,
    };

    if tier == "free" {
        let key = mint_key(
            &state,
            customer_id,
            "Default",
            vec!["read".into(), "write".into()],
            None,
        )
        .await?;
        resp.api_key = Some(key);
    } else if let Some(price_id) = state.stripe.price_for_tier(&tier) {
        match stripe::create_checkout_session(
            &state.stripe,
            price_id,
            &email,
            // Stripe `client_reference_id` carries the customer urn back
            // through checkout.session.completed so the webhook handler
            // can resolve it without exposing the internal id.
            &urn.to_string(),
        )
        .await
        {
            Ok(sess) => resp.checkout_url = sess.url,
            Err(e) => {
                warn!(error=%e, "stripe checkout failed; account created on free tier");
                let key = mint_key(
                    &state,
                    customer_id,
                    "Default",
                    vec!["read".into(), "write".into()],
                    None,
                )
                .await?;
                resp.api_key = Some(key);
            }
        }
    } else {
        warn!(tier=%tier, "stripe price unset; signing up as free tier");
        let key = mint_key(
            &state,
            customer_id,
            "Default",
            vec!["read".into(), "write".into()],
            None,
        )
        .await?;
        resp.api_key = Some(key);
    }

    info!(customer_urn=%urn, tier=%tier, "customer created");
    Ok(Json(resp))
}

/// Per-tier limit on active API keys.
fn key_limit_for_tier(tier: &str) -> i64 {
    match tier {
        "free" => 2,
        "pro" => 25,
        "business" | "enterprise" => 100,
        _ => 2,
    }
}

async fn mint_key(
    state: &AppState,
    customer_id: i64,
    name: &str,
    scopes: Vec<String>,
    expires_at: Option<chrono::DateTime<Utc>>,
) -> Result<String, ApiError> {
    let g = gen_key(false);
    let new_key = NewApiKey {
        urn: Uuid::new_v4(),
        customer_id,
        name: name.to_string(),
        key_hash: g.hash,
        key_prefix: g.prefix,
        status_code: "active".into(),
        scopes,
        expires_at,
    };
    state.repo.insert_api_key(&new_key).await?;
    Ok(g.raw)
}

// ---------------------------------------------------------------------------
// Signin
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct SigninReq {
    email: String,
    password: String,
}

#[derive(Serialize)]
struct SigninResp {
    customer_urn: String,
    session_token: String,
    tier: String,
    name: Option<String>,
    company: Option<String>,
}

async fn signin(
    State(state): State<AppState>,
    Json(req): Json<SigninReq>,
) -> Result<Json<SigninResp>, ApiError> {
    let email = req.email.trim().to_lowercase();
    let customer = state
        .repo
        .find_customer_by_email(&email)
        .await?
        .ok_or_else(|| ApiError(StatusCode::UNAUTHORIZED, "invalid credentials".into()))?;
    if !verify_password(&req.password, &customer.password_hash) {
        return Err(ApiError(
            StatusCode::UNAUTHORIZED,
            "invalid credentials".into(),
        ));
    }
    let token = issue_session(customer.id, &state.session_secret);
    Ok(Json(SigninResp {
        customer_urn: customer.urn.to_string(),
        session_token: token,
        tier: customer.tier_code,
        name: customer.name,
        company: customer.company,
    }))
}

// ---------------------------------------------------------------------------
// Account
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct MeResp {
    urn: String,
    email: String,
    name: Option<String>,
    company: Option<String>,
    tier: String,
    subscription_status: String,
    subscription_type: String,
    has_stripe_customer: bool,
}

async fn me(State(state): State<AppState>, headers: HeaderMap) -> Result<Json<MeResp>, ApiError> {
    let c = require_session(&state, &headers).await?;
    Ok(Json(MeResp {
        urn: c.urn.to_string(),
        email: c.email,
        name: c.name,
        company: c.company,
        tier: c.tier_code,
        subscription_status: c.subscription_status_code,
        subscription_type: c.subscription_type_code,
        has_stripe_customer: c.stripe_customer_id.is_some(),
    }))
}

#[derive(Serialize)]
struct ListedKey {
    urn: String,
    name: String,
    prefix: String,
    status: String,
    scopes: Vec<String>,
    created_at: String,
    last_used_at: Option<String>,
    expires_at: Option<String>,
}

async fn list_keys(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let c = require_session(&state, &headers).await?;
    let keys = state.repo.list_api_keys(c.id).await?;
    let listed: Vec<ListedKey> = keys
        .into_iter()
        .map(|k| ListedKey {
            urn: k.urn.to_string(),
            name: k.name,
            prefix: k.key_prefix,
            status: k.status_code,
            scopes: k.scopes,
            created_at: k.created_at.to_rfc3339(),
            last_used_at: k.last_used_at.map(|t| t.to_rfc3339()),
            expires_at: k.expires_at.map(|t| t.to_rfc3339()),
        })
        .collect();
    Ok(Json(json!({ "keys": listed })))
}

#[derive(Deserialize)]
struct CreateKeyReq {
    name: String,
    #[serde(default)]
    scopes: Vec<String>,
    expires_at: Option<String>,
}

async fn create_key(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<CreateKeyReq>,
) -> Result<Json<Value>, ApiError> {
    let c = require_session(&state, &headers).await?;
    if req.name.trim().is_empty() {
        return Err(bad("name required"));
    }

    // Enforce per-tier limit on active keys.
    let limit = key_limit_for_tier(&c.tier_code);
    let active = state.repo.count_active_keys(c.id).await?;
    if active >= limit {
        return Err(ApiError(
            StatusCode::FORBIDDEN,
            format!(
                "key limit reached ({}/{}); upgrade tier for more",
                active, limit
            ),
        ));
    }

    // Default scopes if none provided.
    let scopes = if req.scopes.is_empty() {
        vec!["read".into(), "write".into()]
    } else {
        req.scopes
    };

    let expires_at = req
        .expires_at
        .as_deref()
        .map(|s| {
            chrono::DateTime::parse_from_rfc3339(s)
                .map(|dt| dt.with_timezone(&Utc))
                .map_err(|_| bad("expires_at must be ISO 8601 / RFC 3339"))
        })
        .transpose()?;

    let raw = mint_key(&state, c.id, req.name.trim(), scopes, expires_at).await?;
    Ok(Json(json!({ "key": raw })))
}

async fn revoke_key(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(urn): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let c = require_session(&state, &headers).await?;
    let urn_parsed = Uuid::parse_str(&urn).map_err(|_| bad("bad urn"))?;
    let ok = state.repo.revoke_api_key_by_urn(c.id, urn_parsed).await?;
    if ok {
        Ok(Json(json!({
            "urn": urn,
            "status": "revoked",
            "revoked_at": Utc::now().to_rfc3339()
        })))
    } else {
        // Key exists but is already revoked → 409 Conflict.
        Err(ApiError(StatusCode::CONFLICT, "key already revoked".into()))
    }
}

// ---------------------------------------------------------------------------
// Billing
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct CheckoutReq {
    tier: String,
}

async fn start_checkout(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<CheckoutReq>,
) -> Result<Json<Value>, ApiError> {
    let c = require_session(&state, &headers).await?;
    let price_id = state
        .stripe
        .price_for_tier(&req.tier.to_lowercase())
        .ok_or_else(|| bad("unsupported tier or stripe not configured"))?
        .to_string();
    let sess =
        stripe::create_checkout_session(&state.stripe, &price_id, &c.email, &c.id.to_string())
            .await
            .map_err(|e| ApiError(StatusCode::BAD_GATEWAY, e.to_string()))?;
    Ok(Json(json!({ "url": sess.url })))
}

async fn billing_portal(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let c = require_session(&state, &headers).await?;
    let scid = c
        .stripe_customer_id
        .ok_or_else(|| bad("no stripe customer on file"))?;
    let sess = stripe::create_portal_session(&state.stripe, &scid)
        .await
        .map_err(|e| ApiError(StatusCode::BAD_GATEWAY, e.to_string()))?;
    Ok(Json(json!({ "url": sess.url })))
}

// ---------------------------------------------------------------------------
// Admin: provision internal-dev customers
//
// Bearer-token-protected. The public /v1/auth/signup endpoint refuses
// `tier=internal`, so this is the only path to create a customer with
// `subscription_type=internal` + enterprise-tier access. Intended for
// internal company developers who need to exercise the full feature
// matrix without paying.
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct AdminCreateInternalReq {
    email: String,
    password: String,
    name: Option<String>,
    company: Option<String>,
}

#[derive(Serialize)]
struct AdminCreateInternalResp {
    customer_urn: String,
    api_key: String,
    /// Tier the customer was provisioned with — always `enterprise` for
    /// internal accounts so they can exercise every paid feature.
    tier: String,
    subscription_type: String,
}

async fn admin_create_internal(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<AdminCreateInternalReq>,
) -> Result<Json<AdminCreateInternalResp>, ApiError> {
    require_admin(&state, &headers)?;

    let email = req.email.trim().to_lowercase();
    if !email.contains('@') {
        return Err(bad("invalid email"));
    }
    if req.password.len() < 8 {
        return Err(bad("password must be at least 8 characters"));
    }
    if state.repo.find_customer_by_email(&email).await?.is_some() {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "email already registered".into(),
        ));
    }

    let hash = hash_password(&req.password)?;
    let urn = Uuid::new_v4();
    // Internal accounts get enterprise tier + internal subscription type
    // directly — no Stripe involvement.
    let new_customer = NewCustomer {
        urn,
        email: email.clone(),
        password_hash: hash,
        name: req.name.filter(|s| !s.is_empty()),
        company: req.company.filter(|s| !s.is_empty()),
        tier_code: "enterprise".to_string(),
        stripe_customer_id: None,
        stripe_subscription_id: None,
        subscription_status_code: "active".into(),
        subscription_type_code: "internal".into(),
    };
    let customer_id = state.repo.create_customer(&new_customer).await?;

    let raw = mint_key(
        &state,
        customer_id,
        "Internal Default",
        vec!["read".into(), "write".into(), "admin".into()],
        None,
    )
    .await?;

    info!(customer_urn=%urn, "admin provisioned internal customer");
    Ok(Json(AdminCreateInternalResp {
        customer_urn: urn.to_string(),
        api_key: raw,
        tier: "enterprise".into(),
        subscription_type: "internal".into(),
    }))
}

// ---------------------------------------------------------------------------
// Stripe webhook
// ---------------------------------------------------------------------------

async fn stripe_webhook(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<StatusCode, ApiError> {
    let sig = headers
        .get("stripe-signature")
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| bad("missing signature"))?;
    let event = stripe::verify_webhook(&state.stripe.webhook_secret, &body, sig).map_err(|e| {
        warn!(error=%e, "webhook verify failed");
        ApiError(StatusCode::UNAUTHORIZED, "bad signature".into())
    })?;

    let kind = event.get("type").and_then(|v| v.as_str()).unwrap_or("");
    let data = event
        .get("data")
        .and_then(|v| v.get("object"))
        .cloned()
        .unwrap_or(Value::Null);

    match kind {
        // Initial subscription completed via Checkout. Promote tier and
        // mint the customer's first live API key.
        "checkout.session.completed" => handle_checkout_completed(&state, &data).await?,
        // Renewal / plan change. Reflect tier from the active line item.
        "customer.subscription.updated" => {
            handle_subscription_event(&state, &data, "active").await?
        }
        // Cancellation. Mark inactive and revoke all API keys.
        "customer.subscription.deleted" => {
            handle_subscription_event(&state, &data, "canceled").await?
        }
        // Payment failed → past_due (still active grace, but flagged).
        "invoice.payment_failed" => handle_payment_failed(&state, &data).await?,
        _ => info!(event_type = %kind, "unhandled stripe event"),
    }
    Ok(StatusCode::OK)
}

async fn handle_checkout_completed(state: &AppState, obj: &Value) -> Result<(), ApiError> {
    // We pass the customer's urn (not the internal id) into Stripe's
    // `client_reference_id` at checkout time. Resolve it back to the
    // internal id before any DB writes.
    let client_ref = obj
        .get("client_reference_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| bad("missing client_reference_id"))?;
    let customer_urn = Uuid::parse_str(client_ref).map_err(|_| bad("bad client_reference_id"))?;
    let customer = state
        .repo
        .find_customer_by_urn(customer_urn)
        .await?
        .ok_or_else(|| bad("customer not found for urn"))?;
    let stripe_customer = obj
        .get("customer")
        .and_then(|v| v.as_str())
        .map(String::from);
    let stripe_sub = obj
        .get("subscription")
        .and_then(|v| v.as_str())
        .map(String::from);

    // Look up the subscription to determine tier.
    let mut tier = "pro".to_string();
    if let Some(sub_id) = &stripe_sub {
        if let Ok(sub) = fetch_subscription(&state.stripe, sub_id).await {
            if let Some(price_id) = sub
                .pointer("/items/data/0/price/id")
                .and_then(|v| v.as_str())
            {
                if let Some(t) = stripe::tier_for_price(&state.stripe, price_id) {
                    tier = t.to_string();
                }
            }
        }
    }

    state
        .repo
        .update_customer_stripe(
            customer.id,
            stripe_customer.as_deref(),
            stripe_sub.as_deref(),
            &tier,
            "active",
        )
        .await?;

    // Promote subscription type from "free" to "paid" now that Stripe
    // checkout succeeded.
    state
        .repo
        .update_customer_subscription_type(customer.id, "paid")
        .await?;

    let existing = state.repo.list_api_keys(customer.id).await?;
    if !existing.iter().any(|k| k.status_code == "active") {
        let g = gen_key(false);
        let new_key = NewApiKey {
            urn: Uuid::new_v4(),
            customer_id: customer.id,
            name: "Default (auto-provisioned)".to_string(),
            key_hash: g.hash,
            key_prefix: g.prefix,
            status_code: "active".into(),
            scopes: vec!["read".into(), "write".into()],
            expires_at: None,
        };
        state.repo.insert_api_key(&new_key).await?;
        info!(customer_urn=%customer.urn, tier=%tier, "minted key after checkout");
    } else {
        info!(customer_urn=%customer.urn, tier=%tier, "subscription activated; existing key kept");
    }
    Ok(())
}

async fn handle_subscription_event(
    state: &AppState,
    obj: &Value,
    new_status: &str,
) -> Result<(), ApiError> {
    let stripe_customer = match obj.get("customer").and_then(|v| v.as_str()) {
        Some(s) => s,
        None => return Ok(()),
    };
    let customer = match state.repo.find_customer_by_stripe(stripe_customer).await? {
        Some(c) => c,
        None => return Ok(()),
    };

    let tier = obj
        .pointer("/items/data/0/price/id")
        .and_then(|v| v.as_str())
        .and_then(|p| stripe::tier_for_price(&state.stripe, p))
        .unwrap_or_else(|| {
            if new_status == "canceled" {
                "free"
            } else {
                customer.tier_code.as_str().into()
            }
        });

    state
        .repo
        .update_customer_stripe(customer.id, None, None, tier, new_status)
        .await?;

    if new_status == "canceled" {
        state.repo.revoke_all_for_customer(customer.id).await?;
        info!(customer_id=%customer.id, "subscription canceled; keys revoked");
    } else {
        info!(customer_id=%customer.id, tier=%tier, status=%new_status, "subscription updated");
    }
    Ok(())
}

async fn handle_payment_failed(state: &AppState, obj: &Value) -> Result<(), ApiError> {
    let stripe_customer = match obj.get("customer").and_then(|v| v.as_str()) {
        Some(s) => s,
        None => return Ok(()),
    };
    if let Some(c) = state.repo.find_customer_by_stripe(stripe_customer).await? {
        state
            .repo
            .update_customer_stripe(c.id, None, None, &c.tier_code, "past_due")
            .await?;
        warn!(customer_id=%c.id, "payment failed -> past_due");
    }
    Ok(())
}

async fn fetch_subscription(
    cfg: &stripe::StripeConfig,
    sub_id: &str,
) -> Result<Value, anyhow::Error> {
    let client = reqwest::Client::new();
    let resp = client
        .get(format!("https://api.stripe.com/v1/subscriptions/{sub_id}"))
        .basic_auth(&cfg.secret_key, Some(""))
        .send()
        .await?;
    let body = resp.text().await?;
    Ok(serde_json::from_str(&body)?)
}

#[allow(dead_code)]
fn _unused_mut_param_silence(_h: &HashMap<String, String>) {}
