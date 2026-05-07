//! Lynxx billing service — separate binary from the gateway.
//!
//! Hosts:
//! - `POST /v1/auth/signup`        — create account, optionally start Stripe checkout
//! - `POST /v1/auth/signin`        — issue session token
//! - `GET  /v1/account/me`         — current customer (Bearer session token)
//! - `GET  /v1/account/keys`       — list API keys
//! - `POST /v1/account/keys`       — create a new API key (returns full key once)
//! - `POST /v1/account/keys/:id/revoke` — revoke a key
//! - `GET  /v1/license/verify`     — public; the gateway calls this on startup
//! - `POST /v1/billing/checkout`   — start Stripe checkout for paid tier
//! - `GET  /v1/billing/portal`     — Stripe customer portal URL
//! - `POST /v1/stripe/webhook`     — Stripe → us, mints/revokes keys on subscription events
//!
//! No licence gating on this binary itself (it's the licence server).

pub mod auth;
pub mod keys;
pub mod models;
pub mod repo;
pub mod server;
pub mod signing;
pub mod stripe;

use axum::http::HeaderValue;
use std::sync::Arc;

#[derive(Clone)]
pub struct AppState {
    pub repo: Arc<dyn repo::BillingRepo>,
    pub session_secret: Vec<u8>,
    /// Master secret for the `/v1/admin/*` endpoints. Compared in constant
    /// time against the bearer token. Provisioning internal-dev customers
    /// requires this — there is no public path to do it.
    pub admin_api_key: String,
    pub stripe: stripe::StripeConfig,
    pub website_origin: String,
    /// Browser origins permitted to send credentialed (cookie-bearing)
    /// requests. CORS reflects the request Origin only when it appears
    /// here; everything else is rejected by the browser. CLI / curl
    /// users bypass this entirely (no Origin header).
    pub allowed_origins: Vec<HeaderValue>,
    /// GCS V4 signed-URL minter for the binaries bucket. None when the
    /// service runs locally without a GCS bucket configured (`/v1/install/binary`
    /// returns 503 in that mode).
    pub gcs: Option<signing::GcsSigner>,
}
