//! Minimal Stripe HTTP client (no async-stripe dep). Just the three
//! operations we need: create checkout session, create billing portal
//! session, and verify webhook signatures.

use anyhow::{anyhow, Result};
use hmac::{Hmac, Mac};
use serde::Deserialize;
use sha2::Sha256;
use std::collections::HashMap;
use std::time::SystemTime;

type HmacSha256 = Hmac<Sha256>;

const STRIPE_API: &str = "https://api.stripe.com/v1";
const SIGNATURE_TOLERANCE_SECS: i64 = 5 * 60;

#[derive(Clone, Debug)]
pub struct StripeConfig {
    pub secret_key: String,
    pub webhook_secret: String,
    pub price_pro: String,
    pub price_business: String,
    pub success_url: String,
    pub cancel_url: String,
    pub portal_return_url: String,
}

impl StripeConfig {
    pub fn enabled(&self) -> bool {
        !self.secret_key.is_empty()
    }

    pub fn price_for_tier(&self, tier: &str) -> Option<&str> {
        let id = match tier {
            "pro" => &self.price_pro,
            "business" => &self.price_business,
            _ => return None,
        };
        if id.is_empty() {
            None
        } else {
            Some(id.as_str())
        }
    }
}

#[derive(Deserialize)]
pub struct CheckoutSession {
    pub id: String,
    pub url: Option<String>,
    pub customer: Option<String>,
}

pub async fn create_checkout_session(
    cfg: &StripeConfig,
    price_id: &str,
    customer_email: &str,
    client_reference_id: &str,
) -> Result<CheckoutSession> {
    if !cfg.enabled() {
        return Err(anyhow!("stripe not configured"));
    }
    let mut form: HashMap<&str, String> = HashMap::new();
    form.insert("mode", "subscription".into());
    form.insert("line_items[0][price]", price_id.into());
    form.insert("line_items[0][quantity]", "1".into());
    form.insert("customer_email", customer_email.into());
    form.insert("client_reference_id", client_reference_id.into());
    form.insert("success_url", format!("{}?session_id={{CHECKOUT_SESSION_ID}}", cfg.success_url));
    form.insert("cancel_url", cfg.cancel_url.clone());
    form.insert("allow_promotion_codes", "true".into());

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{STRIPE_API}/checkout/sessions"))
        .basic_auth(&cfg.secret_key, Some(""))
        .form(&form)
        .send()
        .await?;

    let status = resp.status();
    let body = resp.text().await?;
    if !status.is_success() {
        return Err(anyhow!("stripe checkout {}: {}", status, body));
    }
    serde_json::from_str(&body).map_err(|e| anyhow!("parse checkout: {e} body={body}"))
}

#[derive(Deserialize)]
pub struct PortalSession {
    pub url: String,
}

pub async fn create_portal_session(
    cfg: &StripeConfig,
    stripe_customer_id: &str,
) -> Result<PortalSession> {
    if !cfg.enabled() {
        return Err(anyhow!("stripe not configured"));
    }
    let mut form: HashMap<&str, String> = HashMap::new();
    form.insert("customer", stripe_customer_id.into());
    form.insert("return_url", cfg.portal_return_url.clone());

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{STRIPE_API}/billing_portal/sessions"))
        .basic_auth(&cfg.secret_key, Some(""))
        .form(&form)
        .send()
        .await?;

    let status = resp.status();
    let body = resp.text().await?;
    if !status.is_success() {
        return Err(anyhow!("stripe portal {}: {}", status, body));
    }
    serde_json::from_str(&body).map_err(|e| anyhow!("parse portal: {e}"))
}

/// Verify the `Stripe-Signature` header against a raw request body,
/// then return the parsed event payload as JSON.
///
/// Header format: `t=<unix>,v1=<hex hmac>[,v0=...]` — we check v1.
pub fn verify_webhook(secret: &str, body: &[u8], header: &str) -> Result<serde_json::Value> {
    let mut t: Option<i64> = None;
    let mut v1: Option<String> = None;
    for pair in header.split(',') {
        let (k, v) = pair.split_once('=').ok_or_else(|| anyhow!("bad sig pair"))?;
        match k {
            "t" => t = v.parse().ok(),
            "v1" => v1 = Some(v.to_string()),
            _ => {}
        }
    }
    let t = t.ok_or_else(|| anyhow!("missing t"))?;
    let v1 = v1.ok_or_else(|| anyhow!("missing v1"))?;

    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_err(|e| anyhow!("clock: {e}"))?
        .as_secs() as i64;
    if (now - t).abs() > SIGNATURE_TOLERANCE_SECS {
        return Err(anyhow!("signature timestamp out of tolerance"));
    }

    let mut signed = Vec::with_capacity(body.len() + 32);
    signed.extend_from_slice(t.to_string().as_bytes());
    signed.push(b'.');
    signed.extend_from_slice(body);

    let mut mac = HmacSha256::new_from_slice(secret.as_bytes())?;
    mac.update(&signed);
    let expected = hex::encode(mac.finalize().into_bytes());

    if !constant_time_eq(expected.as_bytes(), v1.as_bytes()) {
        return Err(anyhow!("bad signature"));
    }
    Ok(serde_json::from_slice(body)?)
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff: u8 = 0;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// Map a Stripe price ID back to a tier label, if it matches one we
/// know about.
pub fn tier_for_price(cfg: &StripeConfig, price_id: &str) -> Option<&'static str> {
    if !cfg.price_pro.is_empty() && cfg.price_pro == price_id {
        Some("pro")
    } else if !cfg.price_business.is_empty() && cfg.price_business == price_id {
        Some("business")
    } else {
        None
    }
}
