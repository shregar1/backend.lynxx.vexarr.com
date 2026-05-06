//! Password hashing (argon2id) + signed session tokens (HMAC-SHA256).
//! Sessions are stateless: token = base64url(customer_id|expires_at).hmac.
//! No DB lookup per request.

use anyhow::{anyhow, Result};
use argon2::password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, SaltString};
use argon2::{Argon2, PasswordVerifier};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use chrono::{Duration, Utc};
use hmac::{Hmac, Mac};
use sha2::Sha256;

const SESSION_VALID_DAYS: i64 = 30;
type HmacSha256 = Hmac<Sha256>;

pub fn hash_password(plain: &str) -> Result<String> {
    let salt = SaltString::generate(&mut OsRng);
    let hash = Argon2::default()
        .hash_password(plain.as_bytes(), &salt)
        .map_err(|e| anyhow!("argon2 hash: {e}"))?
        .to_string();
    Ok(hash)
}

pub fn verify_password(plain: &str, hash: &str) -> bool {
    let parsed = match PasswordHash::new(hash) {
        Ok(h) => h,
        Err(_) => return false,
    };
    Argon2::default()
        .verify_password(plain.as_bytes(), &parsed)
        .is_ok()
}

/// Issue a session token for the given customer. Payload format:
/// `<customer_id_i64>|<expires_at_unix>`, signed with HMAC-SHA256.
pub fn issue_session(customer_id: i64, secret: &[u8]) -> String {
    let exp = (Utc::now() + Duration::days(SESSION_VALID_DAYS)).timestamp();
    let payload = format!("{}|{}", customer_id, exp);
    let mut mac = HmacSha256::new_from_slice(secret).expect("HMAC accepts any key length");
    mac.update(payload.as_bytes());
    let sig = mac.finalize().into_bytes();
    format!(
        "{}.{}",
        URL_SAFE_NO_PAD.encode(payload.as_bytes()),
        URL_SAFE_NO_PAD.encode(sig)
    )
}

pub fn verify_session(token: &str, secret: &[u8]) -> Option<i64> {
    let (payload_b64, sig_b64) = token.split_once('.')?;
    let payload = URL_SAFE_NO_PAD.decode(payload_b64.as_bytes()).ok()?;
    let sig = URL_SAFE_NO_PAD.decode(sig_b64.as_bytes()).ok()?;

    let mut mac = HmacSha256::new_from_slice(secret).ok()?;
    mac.update(&payload);
    mac.verify_slice(&sig).ok()?;

    let payload_str = std::str::from_utf8(&payload).ok()?;
    let (id_str, exp_str) = payload_str.split_once('|')?;
    let exp: i64 = exp_str.parse().ok()?;
    if Utc::now().timestamp() > exp {
        return None;
    }
    id_str.parse::<i64>().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn password_round_trip() {
        let h = hash_password("hunter2").unwrap();
        assert!(verify_password("hunter2", &h));
        assert!(!verify_password("wrong", &h));
    }

    #[test]
    fn session_round_trip() {
        let secret = b"test-secret-32-bytes-long-padding";
        let id: i64 = 42;
        let token = issue_session(id, secret);
        assert_eq!(verify_session(&token, secret), Some(id));
    }

    #[test]
    fn session_rejects_tampered() {
        let secret = b"test-secret-32-bytes-long-padding";
        let id: i64 = 42;
        let token = issue_session(id, secret);
        let bad = format!("{}x", token);
        assert_eq!(verify_session(&bad, secret), None);
    }

    #[test]
    fn session_rejects_wrong_secret() {
        let id: i64 = 42;
        let token = issue_session(id, b"secret-a");
        assert_eq!(verify_session(&token, b"secret-b"), None);
    }
}
