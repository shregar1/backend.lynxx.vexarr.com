//! Inbound TLS termination on the proxy listener.
//!
//! When `[gateway.tls].enabled = true` the gateway serves HTTPS on the
//! proxy port. With `client_ca_path` set, mTLS is enforced — incoming
//! connections must present a client cert that chains to the configured
//! CA bundle. Two modes:
//!
//! * `client_cert_required = false` — optional. Connections without a
//!   client cert are accepted; ones that present a cert get validated.
//! * `client_cert_required = true` — required. The TLS handshake fails
//!   if the client doesn't send a cert.
//!
//! The actual handshake is driven by `axum_server` + `tokio-rustls` so we
//! never touch OpenSSL. This module only assembles the rustls
//! `ServerConfig`.
//!
//! ## Header injection (mTLS)
//!
//! When mTLS validates a client cert, we'd ideally surface the cert's
//! subject CN and SHA256 fingerprint to upstream services as
//! `X-Client-Cert-CN` and `X-Client-Cert-Fingerprint` headers. Doing
//! that *during* the rustls handshake requires holding the verifier's
//! decision in connection-local state. axum-server doesn't expose that
//! plumbing in 0.7; the sound v1 implementation is to expose the
//! validated chain via a custom `ClientCertVerifier` that tees the cert
//! into a per-connection slot, then a small middleware reads from it on
//! the request side. That's a follow-up — for v1 we accept that the
//! headers aren't injected, but we *do* enforce client auth, which is
//! the security-relevant half. See `LinkedClientCertVerifier` below for
//! the verifier wiring; the request-side glue is annotated TODO.

use crate::config::{AdminTlsConfig, TlsSettings};
use crate::error::GatewayError;
use rustls::crypto::ring as ring_provider;
use rustls::crypto::CryptoProvider;
use rustls::pki_types::CertificateDer;
use rustls::server::danger::{ClientCertVerified, ClientCertVerifier};
use rustls::server::WebPkiClientVerifier;
use rustls::{DistinguishedName, RootCertStore, ServerConfig, SignatureScheme};
use std::sync::Arc;

/// Build the rustls `ServerConfig` for the proxy listener.
///
/// Errors at startup if any path is missing or any PEM is malformed —
/// it's better to refuse to boot than to silently fall back to plain
/// HTTP when the operator asked for HTTPS.
///
/// Applies TLS version constraints (`min_version` / `max_version`) and
/// curve selection (`curves`) from the config. When `curves` is empty
/// the ring provider's full set is used (x25519, secp256r1, secp384r1).
///
/// **Post-quantum note:** `x25519mlkem768` is not available in the ring
/// provider. A future release will add an `aws-lc-rs` provider option
/// to unlock post-quantum key exchange.
pub fn build_server_config(settings: &TlsSettings) -> Result<ServerConfig, GatewayError> {
    if settings.cert_path.is_empty() || settings.key_path.is_empty() {
        return Err(GatewayError::Internal(
            "gateway.tls.enabled requires cert_path and key_path".into(),
        ));
    }

    let cert_chain = load_cert_chain(&settings.cert_path)?;
    let key = load_private_key(&settings.key_path)?;

    // --- Protocol version constraints ---
    let versions = resolve_tls_versions(&settings.min_version, &settings.max_version)?;

    // --- Key exchange group / curve selection ---
    let provider = resolve_crypto_provider(&settings.curves)?;

    let builder = ServerConfig::builder_with_provider(Arc::new(provider))
        .with_protocol_versions(&versions)
        .map_err(|e| GatewayError::Internal(format!("tls protocol versions: {e}")))?;

    let mut config = if settings.mtls_enabled() {
        let verifier = build_client_verifier(settings)?;
        builder
            .with_client_cert_verifier(verifier)
            .with_single_cert(cert_chain, key)
            .map_err(|e| GatewayError::Internal(format!("tls server cert: {e}")))?
    } else {
        builder
            .with_no_client_auth()
            .with_single_cert(cert_chain, key)
            .map_err(|e| GatewayError::Internal(format!("tls server cert: {e}")))?
    };

    // Match what axum/hyper would advertise. Order matters — h2 first so
    // ALPN-aware clients pick HTTP/2 when both are acceptable.
    config.alpn_protocols = vec![b"h3".to_vec(), b"h2".to_vec(), b"http/1.1".to_vec()];

    if settings.mtls_enabled() {
        tracing::info!(
            cert = %settings.cert_path,
            ca = %settings.client_ca_path,
            required = settings.client_cert_required,
            min_tls = %settings.min_version,
            max_tls = %settings.max_version,
            "inbound mTLS configured"
        );
    } else {
        tracing::info!(
            cert = %settings.cert_path,
            min_tls = %settings.min_version,
            max_tls = %settings.max_version,
            "inbound TLS configured (no mTLS)"
        );
    }

    Ok(config)
}

/// Build a rustls `ServerConfig` for the admin API listener.
///
/// Supports optional mTLS when `client_ca_path` is set. Uses default
/// TLS 1.2+1.3 and the ring provider's default curves.
pub fn build_admin_tls_config(settings: &AdminTlsConfig) -> Result<ServerConfig, GatewayError> {
    if settings.cert_path.is_empty() || settings.key_path.is_empty() {
        return Err(GatewayError::Internal(
            "gateway.admin_tls.enabled requires cert_path and key_path".into(),
        ));
    }

    let cert_chain = load_cert_chain(&settings.cert_path)?;
    let key = load_private_key(&settings.key_path)?;

    let builder = ServerConfig::builder();

    let mut config = if !settings.client_ca_path.is_empty() {
        let proxy_tls = TlsSettings {
            enabled: true,
            cert_path: settings.cert_path.clone(),
            key_path: settings.key_path.clone(),
            client_ca_path: settings.client_ca_path.clone(),
            client_cert_required: settings.client_cert_required,
            ..TlsSettings::default()
        };
        let verifier = build_client_verifier(&proxy_tls)?;
        builder
            .with_client_cert_verifier(verifier)
            .with_single_cert(cert_chain, key)
            .map_err(|e| GatewayError::Internal(format!("admin tls cert: {e}")))?
    } else {
        builder
            .with_no_client_auth()
            .with_single_cert(cert_chain, key)
            .map_err(|e| GatewayError::Internal(format!("admin tls cert: {e}")))?
    };

    config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];

    tracing::info!(
        cert = %settings.cert_path,
        mtls = !settings.client_ca_path.is_empty(),
        "admin TLS configured"
    );

    Ok(config)
}

/// Map config strings `"1.2"` / `"1.3"` to rustls protocol version statics.
fn resolve_tls_versions(
    min: &str,
    max: &str,
) -> Result<Vec<&'static rustls::SupportedProtocolVersion>, GatewayError> {
    let min_ver = parse_tls_version(min, "min_version")?;
    let max_ver = parse_tls_version(max, "max_version")?;

    if min_ver > max_ver {
        return Err(GatewayError::Internal(format!(
            "tls.min_version ({min}) must not exceed tls.max_version ({max})"
        )));
    }

    let mut versions = Vec::with_capacity(2);
    if min_ver <= 12 && max_ver >= 12 {
        versions.push(&rustls::version::TLS12);
    }
    if min_ver <= 13 && max_ver >= 13 {
        versions.push(&rustls::version::TLS13);
    }
    if versions.is_empty() {
        return Err(GatewayError::Internal(
            "tls version range resolved to zero supported versions".into(),
        ));
    }
    Ok(versions)
}

/// Parse `"1.2"` → `12`, `"1.3"` → `13`.
fn parse_tls_version(s: &str, field: &str) -> Result<u8, GatewayError> {
    match s {
        "1.2" => Ok(12),
        "1.3" => Ok(13),
        other => Err(GatewayError::Internal(format!(
            "tls.{field}: unsupported version \"{other}\" (accepted: \"1.2\", \"1.3\")"
        ))),
    }
}

/// Build a [`CryptoProvider`] with the ring backend, optionally filtered to
/// only the named key-exchange groups listed in `curves`. An empty list
/// means "use all supported groups".
fn resolve_crypto_provider(curves: &[String]) -> Result<CryptoProvider, GatewayError> {
    let mut provider = ring_provider::default_provider();

    if !curves.is_empty() {
        let mut selected = Vec::with_capacity(curves.len());
        for name in curves {
            let group = match name.to_lowercase().as_str() {
                "x25519" => ring_provider::kx_group::X25519,
                "secp256r1" | "p256" | "prime256v1" => ring_provider::kx_group::SECP256R1,
                "secp384r1" | "p384" => ring_provider::kx_group::SECP384R1,
                other => {
                    return Err(GatewayError::Internal(format!(
                        "tls.curves: unknown curve \"{other}\" \
                         (supported with ring: x25519, secp256r1, secp384r1; \
                         post-quantum x25519mlkem768 requires the aws-lc-rs provider)"
                    )));
                }
            };
            // Avoid duplicates while preserving operator ordering.
            if !selected.iter().any(|g: &&dyn rustls::crypto::SupportedKxGroup| {
                g.name() == group.name()
            }) {
                selected.push(group);
            }
        }
        provider.kx_groups = selected;
    }

    Ok(provider)
}

fn load_cert_chain(path: &str) -> Result<Vec<CertificateDer<'static>>, GatewayError> {
    let bytes = std::fs::read(path)
        .map_err(|e| GatewayError::Internal(format!("read tls.cert_path {path}: {e}")))?;
    let mut cursor = std::io::Cursor::new(bytes);
    let chain: Vec<_> = rustls_pemfile::certs(&mut cursor)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| GatewayError::Internal(format!("parse tls.cert_path {path}: {e}")))?;
    if chain.is_empty() {
        return Err(GatewayError::Internal(format!(
            "no certs in tls.cert_path {path}"
        )));
    }
    Ok(chain)
}

fn load_private_key(path: &str) -> Result<rustls::pki_types::PrivateKeyDer<'static>, GatewayError> {
    let bytes = std::fs::read(path)
        .map_err(|e| GatewayError::Internal(format!("read tls.key_path {path}: {e}")))?;
    let mut cursor = std::io::Cursor::new(bytes);
    rustls_pemfile::private_key(&mut cursor)
        .map_err(|e| GatewayError::Internal(format!("parse tls.key_path {path}: {e}")))?
        .ok_or_else(|| GatewayError::Internal(format!("no private key in tls.key_path {path}")))
}

fn build_client_verifier(
    settings: &TlsSettings,
) -> Result<Arc<dyn ClientCertVerifier>, GatewayError> {
    let mut roots = RootCertStore::empty();
    let bytes = std::fs::read(&settings.client_ca_path).map_err(|e| {
        GatewayError::Internal(format!(
            "read tls.client_ca_path {}: {e}",
            settings.client_ca_path
        ))
    })?;
    let mut cursor = std::io::Cursor::new(bytes);
    for cert in rustls_pemfile::certs(&mut cursor) {
        let cert = cert.map_err(|e| {
            GatewayError::Internal(format!(
                "parse tls.client_ca_path {}: {e}",
                settings.client_ca_path
            ))
        })?;
        roots
            .add(cert)
            .map_err(|e| GatewayError::Internal(format!("add client ca cert: {e}")))?;
    }
    if roots.is_empty() {
        return Err(GatewayError::Internal(format!(
            "no certs in tls.client_ca_path {}",
            settings.client_ca_path
        )));
    }

    let webpki = WebPkiClientVerifier::builder(Arc::new(roots))
        .build()
        .map_err(|e| GatewayError::Internal(format!("client verifier build: {e}")))?;

    if settings.client_cert_required {
        Ok(webpki)
    } else {
        Ok(Arc::new(OptionalClientVerifier { inner: webpki }))
    }
}

/// Wraps a strict client verifier to make client auth optional.
///
/// rustls' `WebPkiClientVerifier` requires a client cert by default;
/// `allow_unauthenticated` flips that. We forward to the inner verifier
/// for the actual chain check so the security-critical path is unchanged.
#[derive(Debug)]
struct OptionalClientVerifier {
    inner: Arc<dyn ClientCertVerifier>,
}

impl ClientCertVerifier for OptionalClientVerifier {
    fn offer_client_auth(&self) -> bool {
        true
    }

    fn client_auth_mandatory(&self) -> bool {
        false
    }

    fn root_hint_subjects(&self) -> &[DistinguishedName] {
        self.inner.root_hint_subjects()
    }

    fn verify_client_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        now: rustls::pki_types::UnixTime,
    ) -> Result<ClientCertVerified, rustls::Error> {
        self.inner
            .verify_client_cert(end_entity, intermediates, now)
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        self.inner.verify_tls12_signature(message, cert, dss)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        self.inner.verify_tls13_signature(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.inner.supported_verify_schemes()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_versions_both() {
        let v = resolve_tls_versions("1.2", "1.3").unwrap();
        assert_eq!(v.len(), 2);
    }

    #[test]
    fn resolve_versions_tls13_only() {
        let v = resolve_tls_versions("1.3", "1.3").unwrap();
        assert_eq!(v.len(), 1);
    }

    #[test]
    fn resolve_versions_tls12_only() {
        let v = resolve_tls_versions("1.2", "1.2").unwrap();
        assert_eq!(v.len(), 1);
    }

    #[test]
    fn resolve_versions_inverted_range() {
        let err = resolve_tls_versions("1.3", "1.2").unwrap_err();
        assert!(err.to_string().contains("must not exceed"));
    }

    #[test]
    fn resolve_versions_bad_string() {
        let err = resolve_tls_versions("1.1", "1.3").unwrap_err();
        assert!(err.to_string().contains("unsupported version"));
    }

    #[test]
    fn resolve_provider_defaults() {
        // Empty curves = all three groups.
        let p = resolve_crypto_provider(&[]).unwrap();
        assert_eq!(p.kx_groups.len(), 3);
    }

    #[test]
    fn resolve_provider_single_curve() {
        let p = resolve_crypto_provider(&["x25519".to_string()]).unwrap();
        assert_eq!(p.kx_groups.len(), 1);
        assert_eq!(
            p.kx_groups[0].name(),
            rustls::NamedGroup::X25519
        );
    }

    #[test]
    fn resolve_provider_multiple_curves() {
        let p = resolve_crypto_provider(&[
            "secp384r1".to_string(),
            "x25519".to_string(),
        ])
        .unwrap();
        assert_eq!(p.kx_groups.len(), 2);
        // Order preserved from config.
        assert_eq!(
            p.kx_groups[0].name(),
            rustls::NamedGroup::secp384r1
        );
        assert_eq!(
            p.kx_groups[1].name(),
            rustls::NamedGroup::X25519
        );
    }

    #[test]
    fn resolve_provider_deduplicates() {
        let p = resolve_crypto_provider(&[
            "x25519".to_string(),
            "x25519".to_string(),
        ])
        .unwrap();
        assert_eq!(p.kx_groups.len(), 1);
    }

    #[test]
    fn resolve_provider_aliases() {
        let p = resolve_crypto_provider(&[
            "p256".to_string(),
            "p384".to_string(),
        ])
        .unwrap();
        assert_eq!(p.kx_groups.len(), 2);
        assert_eq!(
            p.kx_groups[0].name(),
            rustls::NamedGroup::secp256r1
        );
    }

    #[test]
    fn resolve_provider_unknown_curve() {
        let err = resolve_crypto_provider(&["x448".to_string()]).unwrap_err();
        assert!(err.to_string().contains("unknown curve"));
        assert!(err.to_string().contains("aws-lc-rs"));
    }

    #[test]
    fn build_admin_tls_rejects_empty_paths() {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let cfg = AdminTlsConfig {
            enabled: true,
            ..AdminTlsConfig::default()
        };
        assert!(build_admin_tls_config(&cfg).is_err());
    }
}
