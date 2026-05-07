//! TLS-ALPN-01 (RFC 8737) challenge support.
//!
//! TLS-ALPN-01 is the second ACME challenge type that operators can use
//! when port 80 isn't reachable — the validator opens a TLS connection
//! to port 443 with the `acme-tls/1` ALPN protocol, and we reply with a
//! short-lived self-signed certificate that embeds the SHA-256 of the
//! challenge key authorization in a critical extension (OID
//! `1.3.6.1.5.5.7.1.31`, the `id-pe-acmeIdentifier` extension).
//!
//! ## Lynxx wiring
//!
//! The actual cert presentation lives in [`AcmeChallengeResolver`], a
//! `rustls::server::ResolvesServerCert` that wraps the regular cert
//! resolver:
//!
//! * if the negotiated ALPN is `acme-tls/1` *and* we have a challenge
//!   cert for the requested SNI hostname, return that cert
//! * otherwise fall through to the inner resolver (the loaded
//!   `cert.pem` / `key.pem` for the production hostname)
//!
//! Operators register challenges through the admin API, exactly like
//! HTTP-01: `POST /admin/v1/acme/tls-alpn-01-challenges` with a domain +
//! key authorization, then trigger the ACME validator. The challenge is
//! cleared with `DELETE /admin/v1/acme/tls-alpn-01-challenges/{domain}`
//! once validation completes.

use crate::error::GatewayError;
use rcgen::{CertificateParams, CustomExtension, DnType, KeyPair};
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::server::{ClientHello, ResolvesServerCert};
use rustls::sign::CertifiedKey;
use sha2::{Digest, Sha256};
use std::sync::Arc;

/// ALPN protocol identifier defined by RFC 8737.
pub const ACME_TLS_ALPN_NAME: &[u8] = b"acme-tls/1";

/// Build a short-lived self-signed certificate that fulfils the
/// TLS-ALPN-01 challenge for `domain`.
///
/// `key_authorization` is the value the ACME server told us to embed
/// (`token || '.' || JWK thumbprint(account_key)`). We hash it with
/// SHA-256 and put the digest into the `acmeIdentifier` X.509 extension
/// — that's the bit the validator inspects.
pub fn build_challenge_cert(
    domain: &str,
    key_authorization: &str,
) -> Result<CertifiedKey, GatewayError> {
    if domain.is_empty() {
        return Err(GatewayError::Internal(
            "TLS-ALPN-01 challenge requires a non-empty domain".into(),
        ));
    }

    // RFC 8737 §3: the extension value is an OCTET STRING containing the
    // SHA-256 digest of the key authorization — 32 bytes wrapped in DER
    // OCTET STRING (tag 0x04, length 0x20).
    let digest = Sha256::digest(key_authorization.as_bytes());
    let mut extension_value = Vec::with_capacity(2 + digest.len());
    extension_value.push(0x04); // OCTET STRING
    extension_value.push(digest.len() as u8); // 32
    extension_value.extend_from_slice(&digest);

    let mut params = CertificateParams::new(vec![domain.to_string()])
        .map_err(|e| GatewayError::Internal(format!("alpn cert params: {e}")))?;
    params
        .distinguished_name
        .push(DnType::CommonName, domain.to_string());
    params
        .distinguished_name
        .push(DnType::OrganizationName, "Lynxx ACME TLS-ALPN-01");

    // OID 1.3.6.1.5.5.7.1.31 (id-pe-acmeIdentifier).
    let mut ext = CustomExtension::from_oid_content(&[1, 3, 6, 1, 5, 5, 7, 1, 31], extension_value);
    ext.set_criticality(true);
    params.custom_extensions.push(ext);

    let key_pair = KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256)
        .map_err(|e| GatewayError::Internal(format!("alpn keypair: {e}")))?;

    let cert = params
        .self_signed(&key_pair)
        .map_err(|e| GatewayError::Internal(format!("alpn self-sign: {e}")))?;

    let cert_der = CertificateDer::from(cert.der().to_vec());
    let key_der = PrivateKeyDer::try_from(key_pair.serialize_der())
        .map_err(|e| GatewayError::Internal(format!("alpn key DER: {e}")))?;

    let signing_key = rustls::crypto::ring::sign::any_supported_type(&key_der)
        .map_err(|e| GatewayError::Internal(format!("alpn signing key: {e}")))?;

    Ok(CertifiedKey::new(vec![cert_der], signing_key))
}

/// rustls cert resolver that presents TLS-ALPN-01 challenge certs when
/// asked, and otherwise delegates to a static "production" cert.
///
/// `inner` is the production cert (cloned in via `Arc`) and is what
/// every non-challenge connection sees. `lookup` is the closure the
/// `AcmeManager` injects so the resolver can ask "do we have a
/// challenge for this SNI?" without holding a hard reference to the
/// manager.
pub struct AcmeChallengeResolver {
    inner: Arc<CertifiedKey>,
    lookup: Box<dyn Fn(&str) -> Option<Arc<CertifiedKey>> + Send + Sync>,
}

impl std::fmt::Debug for AcmeChallengeResolver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AcmeChallengeResolver").finish()
    }
}

impl AcmeChallengeResolver {
    pub fn new<F>(inner: Arc<CertifiedKey>, lookup: F) -> Self
    where
        F: Fn(&str) -> Option<Arc<CertifiedKey>> + Send + Sync + 'static,
    {
        Self {
            inner,
            lookup: Box::new(lookup),
        }
    }
}

impl ResolvesServerCert for AcmeChallengeResolver {
    fn resolve(&self, hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        let alpn_match = hello
            .alpn()
            .map(|mut iter| iter.any(|p| p == ACME_TLS_ALPN_NAME))
            .unwrap_or(false);
        if alpn_match {
            if let Some(sni) = hello.server_name() {
                if let Some(cert) = (self.lookup)(sni) {
                    tracing::debug!(domain = %sni, "serving TLS-ALPN-01 challenge cert");
                    return Some(cert);
                }
            }
            tracing::warn!(
                "acme-tls/1 ALPN requested but no matching challenge registered; \
                 falling back to production cert"
            );
        }
        Some(Arc::clone(&self.inner))
    }
}

/// Parse a chain + key on disk into a [`CertifiedKey`]. Used by the
/// AcmeManager when it wants to wrap the production cert behind a
/// challenge-aware resolver.
pub fn certified_key_from_pem(
    cert_pem: &[u8],
    key_pem: &[u8],
) -> Result<CertifiedKey, GatewayError> {
    let mut cur = std::io::Cursor::new(cert_pem);
    let certs: Vec<CertificateDer<'static>> = rustls_pemfile::certs(&mut cur)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| GatewayError::Internal(format!("parse cert pem: {e}")))?;
    if certs.is_empty() {
        return Err(GatewayError::Internal("no certs in pem".into()));
    }
    let mut cur = std::io::Cursor::new(key_pem);
    let key = rustls_pemfile::private_key(&mut cur)
        .map_err(|e| GatewayError::Internal(format!("parse key pem: {e}")))?
        .ok_or_else(|| GatewayError::Internal("no private key in pem".into()))?;
    let signing_key = rustls::crypto::ring::sign::any_supported_type(&key)
        .map_err(|e| GatewayError::Internal(format!("signing key: {e}")))?;
    Ok(CertifiedKey::new(certs, signing_key))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn install_provider() {
        let _ = rustls::crypto::ring::default_provider().install_default();
    }

    #[test]
    fn builds_challenge_cert_with_acme_extension() {
        install_provider();
        let ck = build_challenge_cert("example.com", "tok.thumbprint").unwrap();
        assert_eq!(ck.cert.len(), 1, "challenge cert is single-leaf");

        // Sanity check: the leaf must contain the acmeIdentifier OID
        // (`1.3.6.1.5.5.7.1.31`) inside its extensions.
        let der = &ck.cert[0];
        let (_, parsed) = x509_parser::parse_x509_certificate(der.as_ref()).unwrap();
        let oid_str = "1.3.6.1.5.5.7.1.31";
        let has_oid = parsed
            .extensions()
            .iter()
            .any(|e| e.oid.to_id_string() == oid_str);
        assert!(has_oid, "challenge cert must carry id-pe-acmeIdentifier");
    }

    #[test]
    fn empty_domain_rejected() {
        install_provider();
        assert!(build_challenge_cert("", "auth").is_err());
    }

    #[test]
    fn digest_changes_with_key_auth() {
        install_provider();
        let a = build_challenge_cert("example.com", "auth-1").unwrap();
        let b = build_challenge_cert("example.com", "auth-2").unwrap();
        assert_ne!(a.cert[0].as_ref(), b.cert[0].as_ref());
    }
}
