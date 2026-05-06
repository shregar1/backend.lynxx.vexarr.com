//! License gating — Ed25519-signed subscription tokens.
//!
//! The Lynxx gateway refuses to start without a valid, unexpired licence
//! token. The matching private key lives only on the licence server
//! (`license_private_key.hex` is gitignored on the dev box).
//!
//! ## Token format
//!
//! ```text
//!   <base64url(payload_json)>.<base64url(signature)>
//! ```
//!
//! `payload_json` is a stable canonical JSON serialisation of:
//!
//! ```json
//! {
//!   "v": 1,
//!   "customer_id": "cus_abc",
//!   "tier":        "pro",
//!   "issued_at":   1746500000,
//!   "expires_at":  1749092000
//! }
//! ```
//!
//! Verification:
//! 1. Decode payload + signature.
//! 2. `verifying_key.verify_strict(payload_bytes, &sig)` — strict mode
//!    rejects malleable signatures.
//! 3. Reject if `expires_at <= now()`.
//! 4. Reject if version is not 1.
//!
//! Loading order at startup:
//! 1. `LYNXX_LICENSE` env var (raw token).
//! 2. `[license].token` field in `config.toml`.
//! 3. None — gateway refuses to start.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use chrono::{DateTime, Utc};
use ed25519_dalek::{Signature, VerifyingKey, SIGNATURE_LENGTH};
use serde::{Deserialize, Serialize};
use std::env;

use crate::quota::PlanTier;

// Embedded Ed25519 public verifying key (32 bytes, hex). The matching
// private key is held only on the licence server. Rotating this key
// invalidates every issued token, so do it deliberately.
const LICENSE_PUBKEY_HEX: &str =
    "e6a4acaf30f2be78eed76c672b5e2a877fe2dcfcfadf3bf8f5295a98d6bd69f0";

const TOKEN_VERSION: u8 = 1;

/// The signed payload inside a licence token. Field names are short to
/// keep tokens compact when copy-pasted into config files.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LicensePayload {
    pub v: u8,
    pub customer_id: String,
    pub tier: String,
    pub issued_at: i64,
    pub expires_at: i64,
}

/// A verified, ready-to-use licence.
#[derive(Debug, Clone)]
pub struct License {
    pub customer_id: String,
    pub tier: PlanTier,
    pub issued_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, thiserror::Error)]
pub enum LicenseError {
    #[error("licence token missing — set LYNXX_LICENSE or [license].token in config.toml")]
    Missing,
    #[error("licence token malformed: {0}")]
    Malformed(String),
    #[error("licence signature invalid")]
    BadSignature,
    #[error("licence expired at {0}")]
    Expired(DateTime<Utc>),
    #[error("licence version {0} not supported by this binary")]
    UnsupportedVersion(u8),
    #[error("licence pubkey misconfigured at build time: {0}")]
    PubkeyConfig(String),
}

impl License {
    /// Verify and decode a raw token.
    pub fn verify(token: &str) -> Result<Self, LicenseError> {
        let (payload_b64, sig_b64) = token
            .split_once('.')
            .ok_or_else(|| LicenseError::Malformed("missing '.' separator".into()))?;

        let payload_bytes = URL_SAFE_NO_PAD
            .decode(payload_b64.as_bytes())
            .map_err(|e| LicenseError::Malformed(format!("payload b64: {e}")))?;
        let sig_bytes = URL_SAFE_NO_PAD
            .decode(sig_b64.as_bytes())
            .map_err(|e| LicenseError::Malformed(format!("signature b64: {e}")))?;

        if sig_bytes.len() != SIGNATURE_LENGTH {
            return Err(LicenseError::Malformed(format!(
                "signature length {} != {}",
                sig_bytes.len(),
                SIGNATURE_LENGTH
            )));
        }
        let sig_arr: [u8; SIGNATURE_LENGTH] = sig_bytes
            .as_slice()
            .try_into()
            .map_err(|_| LicenseError::Malformed("sig array".into()))?;
        let sig = Signature::from_bytes(&sig_arr);

        let pubkey = pubkey()?;
        pubkey
            .verify_strict(&payload_bytes, &sig)
            .map_err(|_| LicenseError::BadSignature)?;

        let payload: LicensePayload = serde_json::from_slice(&payload_bytes)
            .map_err(|e| LicenseError::Malformed(format!("payload json: {e}")))?;

        if payload.v != TOKEN_VERSION {
            return Err(LicenseError::UnsupportedVersion(payload.v));
        }

        let expires_at = DateTime::<Utc>::from_timestamp(payload.expires_at, 0)
            .ok_or_else(|| LicenseError::Malformed("expires_at out of range".into()))?;
        let issued_at = DateTime::<Utc>::from_timestamp(payload.issued_at, 0)
            .ok_or_else(|| LicenseError::Malformed("issued_at out of range".into()))?;

        let now = Utc::now();
        if expires_at <= now {
            return Err(LicenseError::Expired(expires_at));
        }

        Ok(License {
            customer_id: payload.customer_id,
            tier: PlanTier::from_str(&payload.tier),
            issued_at,
            expires_at,
        })
    }

    /// Load a token from env or config and verify it. Hard-fails the
    /// gateway when missing or invalid — call this once at startup.
    pub fn load(config_token: Option<&str>) -> Result<Self, LicenseError> {
        let token = match env::var("LYNXX_LICENSE").ok() {
            Some(t) if !t.trim().is_empty() => t,
            _ => match config_token {
                Some(t) if !t.trim().is_empty() => t.to_string(),
                _ => return Err(LicenseError::Missing),
            },
        };
        Self::verify(token.trim())
    }

    /// Days until the licence expires, clamped at zero.
    pub fn days_remaining(&self) -> i64 {
        (self.expires_at - Utc::now()).num_days().max(0)
    }
}

fn pubkey() -> Result<VerifyingKey, LicenseError> {
    let bytes = hex::decode(LICENSE_PUBKEY_HEX)
        .map_err(|e| LicenseError::PubkeyConfig(format!("hex decode: {e}")))?;
    let arr: [u8; 32] = bytes
        .as_slice()
        .try_into()
        .map_err(|_| LicenseError::PubkeyConfig("pubkey must be 32 bytes".into()))?;
    VerifyingKey::from_bytes(&arr).map_err(|e| LicenseError::PubkeyConfig(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    /// Build a token signed by `sk` with the given payload — used in tests
    /// AND by the licence-issuer tool. Kept inside `cfg(test)` here; the
    /// issuer binary re-implements it because it needs the private key.
    fn issue(sk: &SigningKey, payload: &LicensePayload) -> String {
        let payload_json = serde_json::to_vec(payload).unwrap();
        let sig = sk.sign(&payload_json);
        format!(
            "{}.{}",
            URL_SAFE_NO_PAD.encode(&payload_json),
            URL_SAFE_NO_PAD.encode(sig.to_bytes())
        )
    }

    fn dev_signing_key() -> SigningKey {
        // Hardcoded dev signing key whose pubkey matches LICENSE_PUBKEY_HEX.
        // Keep this in sync with `license_private_key.hex` in the repo root.
        let sk_hex = "2ae9641a216b0d65442bbe653f8ceade3c3138bbbfc642a6bdf41eaa190dfa35";
        let bytes = hex::decode(sk_hex).unwrap();
        let arr: [u8; 32] = bytes.as_slice().try_into().unwrap();
        SigningKey::from_bytes(&arr)
    }

    #[test]
    fn round_trip_valid_token() {
        let sk = dev_signing_key();
        let payload = LicensePayload {
            v: 1,
            customer_id: "cus_test".into(),
            tier: "pro".into(),
            issued_at: Utc::now().timestamp(),
            expires_at: (Utc::now() + chrono::Duration::days(30)).timestamp(),
        };
        let token = issue(&sk, &payload);
        let lic = License::verify(&token).unwrap();
        assert_eq!(lic.customer_id, "cus_test");
        assert_eq!(lic.tier, PlanTier::Pro);
    }

    #[test]
    fn rejects_expired() {
        let sk = dev_signing_key();
        let payload = LicensePayload {
            v: 1,
            customer_id: "cus_test".into(),
            tier: "pro".into(),
            issued_at: 1_000_000_000,
            expires_at: 1_000_000_001,
        };
        let token = issue(&sk, &payload);
        assert!(matches!(
            License::verify(&token),
            Err(LicenseError::Expired(_))
        ));
    }

    #[test]
    fn rejects_tampered_payload() {
        let sk = dev_signing_key();
        let payload = LicensePayload {
            v: 1,
            customer_id: "cus_test".into(),
            tier: "free".into(),
            issued_at: Utc::now().timestamp(),
            expires_at: (Utc::now() + chrono::Duration::days(30)).timestamp(),
        };
        let token = issue(&sk, &payload);
        let (_, sig) = token.split_once('.').unwrap();
        let bad_payload = LicensePayload {
            tier: "enterprise".into(),
            ..payload
        };
        let tampered = format!(
            "{}.{}",
            URL_SAFE_NO_PAD.encode(serde_json::to_vec(&bad_payload).unwrap()),
            sig
        );
        assert!(matches!(
            License::verify(&tampered),
            Err(LicenseError::BadSignature)
        ));
    }

    #[test]
    fn rejects_garbage() {
        assert!(matches!(
            License::verify("not-a-token"),
            Err(LicenseError::Malformed(_))
        ));
    }
}
