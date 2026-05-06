//! `lynxx-license-issue` — sign a Lynxx licence token using the Ed25519
//! private key. Run on the licence server (or locally for dev), never
//! shipped to customers.
//!
//! Usage:
//!   LYNXX_LICENSE_PRIVATE_KEY=<hex64> \
//!     lynxx-license-issue --customer cus_abc --tier pro --days 30
//!
//! The matching public key is hard-coded in `src/license/mod.rs`. Rotate
//! both at the same time or every existing token becomes invalid.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use chrono::{Duration, Utc};
use clap::Parser;
use ed25519_dalek::{Signer, SigningKey};
use serde::Serialize;

#[derive(Parser, Debug)]
#[command(version, about = "Issue a signed Lynxx licence token")]
struct Args {
    /// Customer identifier (Stripe customer id, internal account id, etc.).
    #[arg(long)]
    customer: String,

    /// Plan tier: free | pro | business | enterprise.
    #[arg(long, default_value = "free")]
    tier: String,

    /// Days until the token expires.
    #[arg(long, default_value_t = 30)]
    days: i64,

    /// Hex-encoded 32-byte Ed25519 signing key. Falls back to the
    /// `LYNXX_LICENSE_PRIVATE_KEY` env var.
    #[arg(long, env = "LYNXX_LICENSE_PRIVATE_KEY")]
    private_key: String,
}

#[derive(Serialize)]
struct LicensePayload {
    v: u8,
    customer_id: String,
    tier: String,
    issued_at: i64,
    expires_at: i64,
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    let tier = args.tier.to_lowercase();
    if !["free", "pro", "business", "enterprise"].contains(&tier.as_str()) {
        anyhow::bail!("tier must be one of: free, pro, business, enterprise");
    }

    let sk_bytes = hex::decode(args.private_key.trim())
        .map_err(|e| anyhow::anyhow!("private key hex decode: {e}"))?;
    let sk_arr: [u8; 32] = sk_bytes
        .as_slice()
        .try_into()
        .map_err(|_| anyhow::anyhow!("private key must be 32 bytes (64 hex chars)"))?;
    let sk = SigningKey::from_bytes(&sk_arr);

    let now = Utc::now();
    let payload = LicensePayload {
        v: 1,
        customer_id: args.customer,
        tier,
        issued_at: now.timestamp(),
        expires_at: (now + Duration::days(args.days)).timestamp(),
    };

    let payload_json = serde_json::to_vec(&payload)?;
    let sig = sk.sign(&payload_json);

    let token = format!(
        "{}.{}",
        URL_SAFE_NO_PAD.encode(&payload_json),
        URL_SAFE_NO_PAD.encode(sig.to_bytes())
    );

    println!("{token}");
    Ok(())
}
