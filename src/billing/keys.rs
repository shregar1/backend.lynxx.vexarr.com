//! API key generation. Format: `lnx_live_<28 base32 chars>` (or
//! `lnx_test_…` if the binary is started with --test-mode). The raw
//! key is shown to the user once; only its SHA-256 hex is stored.

use rand::RngCore;
use sha2::{Digest, Sha256};

const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
const KEY_LEN: usize = 28;

pub struct GeneratedKey {
    pub raw: String,
    pub hash: String,
    pub prefix: String,
}

pub fn generate(test_mode: bool) -> GeneratedKey {
    let mut rng = rand::thread_rng();
    let mut buf = vec![0u8; KEY_LEN];
    let mut out = String::with_capacity(KEY_LEN);
    rng.fill_bytes(&mut buf);
    for b in buf {
        out.push(ALPHABET[(b as usize) % ALPHABET.len()] as char);
    }
    let raw = format!(
        "{}{}",
        if test_mode { "lnx_test_" } else { "lnx_live_" },
        out
    );
    let hash = hash_key(&raw);
    let prefix: String = raw.chars().take(16).collect();
    GeneratedKey { raw, hash, prefix }
}

pub fn hash_key(raw: &str) -> String {
    let mut h = Sha256::new();
    h.update(raw.as_bytes());
    format!("{:x}", h.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_format() {
        let k = generate(false);
        assert!(k.raw.starts_with("lnx_live_"));
        assert_eq!(k.raw.len(), "lnx_live_".len() + KEY_LEN);
        assert_eq!(k.hash.len(), 64);
        assert_eq!(k.prefix.len(), 16);
        assert_eq!(hash_key(&k.raw), k.hash);
    }

    #[test]
    fn keys_are_unique() {
        let a = generate(false);
        let b = generate(false);
        assert_ne!(a.raw, b.raw);
        assert_ne!(a.hash, b.hash);
    }
}
