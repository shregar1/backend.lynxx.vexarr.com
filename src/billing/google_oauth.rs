//! Google ID-token verifier for "Sign in with Google".
//!
//! The website obtains an ID token (a signed RS256 JWT) from
//! `accounts.google.com/gsi/client` and POSTs it to
//! `/v1/auth/google`. This module verifies that JWT against Google's
//! published JWKS, then exposes the trusted email / sub / name
//! claims so the handler can find-or-create a customer.
//!
//! What we check (RFC 7519 + Google's docs at
//! <https://developers.google.com/identity/sign-in/web/backend-auth>):
//! - JWT signature against one of Google's RSA public keys (kid lookup).
//! - `iss` is `https://accounts.google.com` or `accounts.google.com`.
//! - `aud` matches our configured OAuth Client ID.
//! - `exp` is in the future (jsonwebtoken does this for us).
//! - `email_verified` is true (we don't trust unverified addresses).
//!
//! JWKS is fetched lazily and cached for an hour. Google rotates
//! signing keys on a similar cadence, so the next miss refreshes.

use anyhow::{anyhow, Result};
use chrono::{DateTime, Duration, Utc};
use jsonwebtoken::{decode, decode_header, jwk::JwkSet, Algorithm, DecodingKey, Validation};
use serde::Deserialize;
use std::sync::Arc;
use tokio::sync::Mutex;

/// Google's OpenID Connect JWKS endpoint.
const JWKS_URL: &str = "https://www.googleapis.com/oauth2/v3/certs";

/// Trusted issuer values. Google publishes either form in legacy /
/// new tokens.
const ISSUERS: &[&str] = &["https://accounts.google.com", "accounts.google.com"];

/// How long to cache the JWKS before re-fetching. Google rotates
/// keys ~daily; an hour is plenty conservative.
const CACHE_TTL: Duration = Duration::hours(1);

#[derive(Debug, Clone, Deserialize)]
pub struct GoogleClaims {
    pub sub: String,
    pub email: String,
    #[serde(default)]
    pub email_verified: bool,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub given_name: Option<String>,
    #[serde(default)]
    pub family_name: Option<String>,
    #[serde(default)]
    pub picture: Option<String>,
    #[serde(default)]
    pub hd: Option<String>,
}

#[derive(Clone)]
pub struct GoogleVerifier {
    /// OAuth 2.0 Client ID configured in the GCP Console. The JWT's
    /// `aud` claim must match this exactly.
    client_id: String,
    cache: Arc<Mutex<Option<CachedJwks>>>,
}

struct CachedJwks {
    keys: JwkSet,
    fetched_at: DateTime<Utc>,
}

impl GoogleVerifier {
    pub fn new(client_id: impl Into<String>) -> Self {
        Self {
            client_id: client_id.into(),
            cache: Arc::new(Mutex::new(None)),
        }
    }

    pub fn client_id(&self) -> &str {
        &self.client_id
    }

    /// Verify a Google-issued ID token and return the trusted claims.
    /// Returns Err with a stable string for any invalid input — the
    /// caller maps that to a 401 without leaking specifics.
    pub async fn verify(&self, id_token: &str) -> Result<GoogleClaims> {
        let header = decode_header(id_token).map_err(|e| anyhow!("bad jwt header: {e}"))?;
        if header.alg != Algorithm::RS256 {
            return Err(anyhow!("unexpected jwt alg: {:?}", header.alg));
        }
        let kid = header
            .kid
            .ok_or_else(|| anyhow!("jwt header missing kid"))?;

        // Try the cached JWKS first; on a miss, refresh once and try
        // again. Avoids storming Google when many users sign in within
        // the cache window, while still picking up new keys promptly.
        let key = match self.lookup_key(&kid, false).await? {
            Some(k) => k,
            None => self
                .lookup_key(&kid, true)
                .await?
                .ok_or_else(|| anyhow!("unknown jwt kid: {kid}"))?,
        };

        let mut validation = Validation::new(Algorithm::RS256);
        validation.set_audience(&[&self.client_id]);
        validation.set_issuer(ISSUERS);
        // Default leeway is 60s; jsonwebtoken validates exp + nbf for us.
        let data = decode::<GoogleClaims>(id_token, &key, &validation)
            .map_err(|e| anyhow!("jwt verify: {e}"))?;
        let claims = data.claims;

        if !claims.email_verified {
            return Err(anyhow!("email_verified=false; refusing to sign in"));
        }
        if claims.email.is_empty() {
            return Err(anyhow!("jwt missing email claim"));
        }
        Ok(claims)
    }

    async fn lookup_key(&self, kid: &str, force_refresh: bool) -> Result<Option<DecodingKey>> {
        let mut guard = self.cache.lock().await;
        let needs_fetch = match guard.as_ref() {
            None => true,
            Some(c) if Utc::now() - c.fetched_at > CACHE_TTL => true,
            _ => force_refresh,
        };
        if needs_fetch {
            let jwks = fetch_jwks().await?;
            *guard = Some(CachedJwks {
                keys: jwks,
                fetched_at: Utc::now(),
            });
        }
        let cache = guard.as_ref().expect("populated above");
        let Some(jwk) = cache.keys.find(kid) else {
            return Ok(None);
        };
        let key = DecodingKey::from_jwk(jwk).map_err(|e| anyhow!("jwk → key: {e}"))?;
        Ok(Some(key))
    }
}

async fn fetch_jwks() -> Result<JwkSet> {
    let client = reqwest::Client::new();
    let resp = client
        .get(JWKS_URL)
        .send()
        .await
        .map_err(|e| anyhow!("jwks fetch: {e}"))?
        .error_for_status()
        .map_err(|e| anyhow!("jwks status: {e}"))?;
    let jwks: JwkSet = resp.json().await.map_err(|e| anyhow!("jwks parse: {e}"))?;
    Ok(jwks)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_garbage_tokens() {
        let v = GoogleVerifier::new("test-client-id");
        // Synchronously confirm header parsing fails fast before any
        // network round-trip.
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(v.verify("not-a-jwt"));
        assert!(result.is_err());
    }
}
