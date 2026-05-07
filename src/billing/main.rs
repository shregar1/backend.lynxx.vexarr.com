//! Lynxx billing service binary — runs the licence/auth/Stripe API.
//! Hosted on api.lynxx.vexarr.com (Cloud Run) in production. Reads
//! its configuration from environment variables.

use anyhow::{anyhow, Context, Result};
use axum::http::HeaderValue;
use lynx_gateway::billing::{
    google_oauth::GoogleVerifier,
    repo::{BillingRepo, SqlBillingRepo},
    server,
    signing::GcsSigner,
    stripe::StripeConfig,
    AppState,
};
use std::env;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::TcpListener;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    // The handler code lives in the `lynx_gateway` library crate, not in
    // the `lynxx_billing` binary, so include both targets in the default
    // filter. Without `lynx_gateway=info` we'd silently drop every
    // `tracing::info/warn/error!` from the request handlers.
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                EnvFilter::new("lynxx_billing=info,lynx_gateway=info,tower_http=info")
            }),
        )
        .init();

    // Connection-string assembly. Two forms are supported:
    //
    //   1. DATABASE_URL set explicitly  →  use as-is. For local sqlite
    //      and dev environments where you want full control of the URL.
    //
    //   2. DB_HOST + DB_USER + DB_NAME (+ DB_PORT, DB_PASSWORD) set →
    //      build a postgres URL. Cloud Run uses this form: DB_HOST is the
    //      Auth Proxy Unix-socket path (`/cloudsql/<connection-name>`),
    //      DB_PASSWORD is injected from Secret Manager. This matches the
    //      shared-instance convention used by every Vexarr product.
    //
    //   3. Neither set → default to a local sqlite file for `cargo run`.
    let database_url = env::var("DATABASE_URL")
        .ok()
        .or_else(build_url_from_db_env)
        .unwrap_or_else(|| "sqlite://lynxx.db?mode=rwc".to_string());
    let bind: SocketAddr = env::var("LYNXX_BILLING_BIND")
        .unwrap_or_else(|_| "0.0.0.0:9100".to_string())
        .parse()
        .context("LYNXX_BILLING_BIND")?;

    let session_secret = env::var("LYNXX_SESSION_SECRET")
        .map(|s| s.into_bytes())
        .map_err(|_| anyhow!("LYNXX_SESSION_SECRET is required (32+ random bytes)"))?;
    if session_secret.len() < 32 {
        return Err(anyhow!("LYNXX_SESSION_SECRET must be at least 32 bytes"));
    }

    // Master admin secret for the /v1/admin/* endpoints. Required — the
    // service refuses to start without one so internal-dev provisioning
    // can never accidentally be left wide open.
    let admin_api_key = env::var("LYNXX_ADMIN_API_KEY")
        .map_err(|_| anyhow!("LYNXX_ADMIN_API_KEY is required (32+ random bytes)"))?;
    if admin_api_key.len() < 32 {
        return Err(anyhow!("LYNXX_ADMIN_API_KEY must be at least 32 bytes"));
    }

    let stripe = StripeConfig {
        secret_key: env::var("STRIPE_SECRET_KEY").unwrap_or_default(),
        webhook_secret: env::var("STRIPE_WEBHOOK_SECRET").unwrap_or_default(),
        price_pro: env::var("STRIPE_PRICE_PRO").unwrap_or_default(),
        price_business: env::var("STRIPE_PRICE_BUSINESS").unwrap_or_default(),
        success_url: env::var("STRIPE_SUCCESS_URL")
            .unwrap_or_else(|_| "https://lynxx.vexarr.com/account/api-keys".into()),
        cancel_url: env::var("STRIPE_CANCEL_URL")
            .unwrap_or_else(|_| "https://lynxx.vexarr.com/pricing".into()),
        portal_return_url: env::var("STRIPE_PORTAL_RETURN_URL")
            .unwrap_or_else(|_| "https://lynxx.vexarr.com/account/billing".into()),
    };
    let website_origin =
        env::var("WEBSITE_ORIGIN").unwrap_or_else(|_| "https://lynxx.vexarr.com".into());

    if !stripe.enabled() {
        tracing::warn!(
            "STRIPE_SECRET_KEY not set — paid checkout disabled, signups fall back to free tier"
        );
    }

    let sql_repo = SqlBillingRepo::connect(&database_url)
        .await
        .context("billing db connect")?;

    // Apply migrations on startup. Idempotent — sqlx tracks applied
    // versions in `_sqlx_migrations`. Picks the right directory based
    // on the URL scheme.
    if database_url.starts_with("postgres") {
        sqlx::migrate!("./migrations/postgres")
            .run(sql_repo.pool())
            .await
            .context("apply postgres migrations")?;
    } else {
        sqlx::migrate!("./migrations/sqlite")
            .run(sql_repo.pool())
            .await
            .context("apply sqlite migrations")?;
    }
    tracing::info!("migrations applied");

    let repo: Arc<dyn BillingRepo> = Arc::new(sql_repo);

    // GCS bucket + prefix for downloadable binaries. Optional — when
    // unset, /v1/install/binary returns 503. Production sets:
    //   BINARIES_BUCKET=vexarr-com
    //   BINARIES_PREFIX=lynxx/binaries
    let gcs = env::var("BINARIES_BUCKET")
        .ok()
        .filter(|s| !s.is_empty())
        .map(|bucket| {
            let prefix = env::var("BINARIES_PREFIX").unwrap_or_default();
            GcsSigner::new(bucket, prefix)
        });
    if let Some(s) = &gcs {
        tracing::info!(bucket = %s.bucket(), prefix = %s.prefix(), "binaries bucket configured");
    } else {
        tracing::warn!("BINARIES_BUCKET not set — /v1/install/binary disabled");
    }

    // Browser origins allowed to send the session cookie. Comma-separated
    // list of full origins (scheme://host[:port]). Defaults cover the
    // production website + Cloud Run URL + local Vite dev. The
    // `website_origin` is always included so a typo'd env var doesn't
    // break the production site.
    let mut origins: Vec<String> = env::var("LYNXX_ALLOWED_ORIGINS")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .map(|s| {
            s.split(',')
                .map(|o| o.trim().trim_end_matches('/').to_string())
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_else(|| {
            vec![
                // Canonical website origin.
                "https://lynxx.vexarr.com".into(),
                "https://www.lynxx.vexarr.com".into(),
                "https://app.lynxx.vexarr.com".into(),
                // Legacy lynxx.dev — kept while DNS / Stripe redirects
                // still point there so existing tabs don't break.
                "https://lynxx.dev".into(),
                "https://www.lynxx.dev".into(),
                "https://app.lynxx.dev".into(),
                // Cloud Run direct URLs (used when no custom domain).
                "https://lynxx-website-588755157227.us-central1.run.app".into(),
                "https://lynxx-website-7jnb4axmhq-uc.a.run.app".into(),
                // Local dev.
                "http://localhost:5173".into(),
                "http://127.0.0.1:5173".into(),
                "http://localhost:4173".into(),
            ]
        });
    let normalized = website_origin.trim_end_matches('/').to_string();
    if !origins.iter().any(|o| o == &normalized) {
        origins.push(normalized);
    }
    let allowed_origins: Vec<HeaderValue> = origins
        .iter()
        .filter_map(|o| match HeaderValue::from_str(o) {
            Ok(h) => Some(h),
            Err(e) => {
                tracing::warn!(origin=%o, error=%e, "ignoring invalid CORS origin");
                None
            }
        })
        .collect();
    tracing::info!(origins = ?origins, "CORS allowlist");

    // Optional Sign-in with Google. The OAuth Client ID is created in
    // GCP Console (see deploy/gcp/google-oauth-setup.md). When unset,
    // /v1/auth/google returns 503 and the website hides the button.
    let google = env::var("GOOGLE_OAUTH_CLIENT_ID")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .map(GoogleVerifier::new);
    if let Some(g) = &google {
        tracing::info!(client_id = %g.client_id(), "google sign-in enabled");
    } else {
        tracing::warn!("GOOGLE_OAUTH_CLIENT_ID unset — /v1/auth/google disabled");
    }

    let state = AppState {
        repo,
        session_secret,
        admin_api_key,
        stripe,
        website_origin,
        allowed_origins,
        gcs,
        google,
    };

    let app = server::router(state);
    let listener = TcpListener::bind(bind).await?;
    tracing::info!(%bind, "lynxx-billing listening");
    axum::serve(listener, app).await?;
    Ok(())
}

/// Build a postgres connection URL from the standard `DB_*` env vars.
/// Returns None if `DB_HOST` is missing — DATABASE_URL or the sqlite
/// default takes over in that case.
///
/// Supports both TCP hosts (e.g. `34.x.x.x`) and unix-socket paths
/// (e.g. `/cloudsql/<conn>`); the latter is encoded via the postgres
/// `host=` query parameter so sqlx routes the connection to the socket
/// instead of trying to resolve `/cloudsql/<conn>` as a TCP hostname.
fn build_url_from_db_env() -> Option<String> {
    let host = env::var("DB_HOST").ok().filter(|s| !s.is_empty())?;
    let user = env::var("DB_USER").unwrap_or_else(|_| "lynxx".to_string());
    let name = env::var("DB_NAME").unwrap_or_else(|_| "lynxx".to_string());
    let port = env::var("DB_PORT").unwrap_or_else(|_| "5432".to_string());
    let pass = env::var("DB_PASSWORD").unwrap_or_default();
    let pass_part = if pass.is_empty() {
        String::new()
    } else {
        // Hand-roll percent-encoding for password chars sqlx + url crate
        // care about (`@`, `:`, `/`, `?`, `#`, `%`, `+`, ` `). Anything
        // else passes through untouched. Avoids pulling in urlencoding
        // for one call site.
        let mut out = String::with_capacity(pass.len() + 8);
        out.push(':');
        for b in pass.bytes() {
            match b {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                    out.push(b as char)
                }
                _ => out.push_str(&format!("%{b:02X}")),
            }
        }
        out
    };

    if host.starts_with('/') {
        // Unix socket — sqlx requires it in the `host` query parameter,
        // not in the host position of the URL.
        Some(format!(
            "postgres://{user}{pass_part}@127.0.0.1/{name}?host={host}"
        ))
    } else {
        Some(format!("postgres://{user}{pass_part}@{host}:{port}/{name}"))
    }
}
