//! Lynxx billing service binary — runs the licence/auth/Stripe API.
//! Hosted on api.lynxx.dev (Cloud Run) in production. Reads its
//! configuration from environment variables.

use anyhow::{anyhow, Context, Result};
use lynx_gateway::billing::{
    repo::{BillingRepo, SqlBillingRepo},
    server,
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
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("lynxx_billing=info,tower_http=info")),
        )
        .init();

    let database_url =
        env::var("DATABASE_URL").unwrap_or_else(|_| "sqlite://lynxx.db?mode=rwc".to_string());
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

    let stripe = StripeConfig {
        secret_key: env::var("STRIPE_SECRET_KEY").unwrap_or_default(),
        webhook_secret: env::var("STRIPE_WEBHOOK_SECRET").unwrap_or_default(),
        price_pro: env::var("STRIPE_PRICE_PRO").unwrap_or_default(),
        price_business: env::var("STRIPE_PRICE_BUSINESS").unwrap_or_default(),
        success_url: env::var("STRIPE_SUCCESS_URL")
            .unwrap_or_else(|_| "https://lynxx.dev/account/api-keys".into()),
        cancel_url: env::var("STRIPE_CANCEL_URL")
            .unwrap_or_else(|_| "https://lynxx.dev/pricing".into()),
        portal_return_url: env::var("STRIPE_PORTAL_RETURN_URL")
            .unwrap_or_else(|_| "https://lynxx.dev/account/billing".into()),
    };
    let website_origin = env::var("WEBSITE_ORIGIN").unwrap_or_else(|_| "https://lynxx.dev".into());

    if !stripe.enabled() {
        tracing::warn!(
            "STRIPE_SECRET_KEY not set — paid checkout disabled, signups fall back to free tier"
        );
    }

    let repo: Arc<dyn BillingRepo> = Arc::new(
        SqlBillingRepo::connect(&database_url)
            .await
            .context("billing db connect")?,
    );

    let state = AppState {
        repo,
        session_secret,
        stripe,
        website_origin,
    };

    let app = server::router(state);
    let listener = TcpListener::bind(bind).await?;
    tracing::info!(%bind, "lynxx-billing listening");
    axum::serve(listener, app).await?;
    Ok(())
}
