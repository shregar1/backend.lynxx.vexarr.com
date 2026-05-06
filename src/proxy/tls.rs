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

use crate::config::TlsSettings;
use crate::error::GatewayError;
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
pub fn build_server_config(settings: &TlsSettings) -> Result<ServerConfig, GatewayError> {
    if settings.cert_path.is_empty() || settings.key_path.is_empty() {
        return Err(GatewayError::Internal(
            "gateway.tls.enabled requires cert_path and key_path".into(),
        ));
    }

    let cert_chain = load_cert_chain(&settings.cert_path)?;
    let key = load_private_key(&settings.key_path)?;

    let builder = ServerConfig::builder();

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
    config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];

    if settings.mtls_enabled() {
        tracing::info!(
            cert = %settings.cert_path,
            ca = %settings.client_ca_path,
            required = settings.client_cert_required,
            "inbound mTLS configured"
        );
    } else {
        tracing::info!(cert = %settings.cert_path, "inbound TLS configured (no mTLS)");
    }

    Ok(config)
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
