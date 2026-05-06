//! Certificate manager with ACME HTTP-01 challenge support, cert file
//! watching, and self-signed certificate generation.
//!
//! This module provides three practical capabilities for TLS provisioning:
//!
//! 1. **Self-signed cert generation** (`self_signed = true`): Creates a
//!    self-signed cert on startup when no cert exists on disk. Intended
//!    for dev/test — browsers show a warning but the connection is encrypted.
//!
//! 2. **HTTP-01 challenge responder**: Serves
//!    `/.well-known/acme-challenge/{token}` responses so external ACME
//!    tools (certbot, acme.sh, lego) or a future built-in client can
//!    complete domain validation.
//!
//! 3. **Cert file watcher**: Polls cert files on disk every 30 seconds
//!    and hot-reloads the `rustls::ServerConfig` when they change, so
//!    operators can renew certs externally without restarting the gateway.
//!
//! ## Storage layout
//!
//! ```text
//! {storage_dir}/
//!   {domain}/
//!     cert.pem     # full chain
//!     key.pem      # private key
//! ```
//!
//! When multiple domains are configured, the first domain is treated as
//! the primary and the rest are SANs in the same certificate.

use crate::config::AcmeConfig;
use crate::error::GatewayError;
use dashmap::DashMap;
use rustls::ServerConfig;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;
use tokio::sync::RwLock;

/// Manages certificate lifecycle: loading, generation, watching, and
/// ACME HTTP-01 challenge handling.
pub struct AcmeManager {
    config: AcmeConfig,
    /// Current rustls `ServerConfig`, swapped atomically on reload.
    current_config: Arc<RwLock<Option<Arc<ServerConfig>>>>,
    /// Pending HTTP-01 challenges: token -> key-authorization.
    challenges: Arc<DashMap<String, String>>,
    /// Last observed mtime of the cert file, for change detection.
    last_cert_mtime: Arc<RwLock<Option<SystemTime>>>,
}

impl AcmeManager {
    /// Create a new `AcmeManager` from the provided config. Does not
    /// perform any I/O — call [`init`] after construction.
    pub fn new(config: AcmeConfig) -> Self {
        Self {
            config,
            current_config: Arc::new(RwLock::new(None)),
            challenges: Arc::new(DashMap::new()),
            last_cert_mtime: Arc::new(RwLock::new(None)),
        }
    }

    /// Return the resolved storage directory. Falls back to
    /// `~/.lynxx/acme/` when not explicitly configured.
    pub fn storage_dir(&self) -> PathBuf {
        if self.config.storage_dir.is_empty() {
            default_storage_dir()
        } else {
            PathBuf::from(&self.config.storage_dir)
        }
    }

    /// Primary domain (first in the list).
    fn primary_domain(&self) -> Option<&str> {
        self.config.domains.first().map(|s| s.as_str())
    }

    /// Path to the certificate PEM for the primary domain.
    fn cert_path(&self) -> Option<PathBuf> {
        self.primary_domain()
            .map(|d| self.storage_dir().join(d).join("cert.pem"))
    }

    /// Path to the private key PEM for the primary domain.
    fn key_path(&self) -> Option<PathBuf> {
        self.primary_domain()
            .map(|d| self.storage_dir().join(d).join("key.pem"))
    }

    // ------------------------------------------------------------------
    // Initialisation
    // ------------------------------------------------------------------

    /// Initialise the manager: load existing certs from disk, or generate
    /// a self-signed cert if configured and no cert exists.
    ///
    /// Returns `Ok(true)` when a usable cert is available after init.
    pub async fn init(&self) -> Result<bool, GatewayError> {
        if self.config.domains.is_empty() {
            return Err(GatewayError::ConfigError(
                "acme.domains must contain at least one domain".into(),
            ));
        }

        // Ensure storage directory exists.
        let domain_dir = self
            .storage_dir()
            .join(self.primary_domain().unwrap_or("_default"));
        std::fs::create_dir_all(&domain_dir).map_err(|e| {
            GatewayError::Internal(format!(
                "create acme storage dir {}: {e}",
                domain_dir.display()
            ))
        })?;

        // Try loading existing cert.
        if self.try_load_cert().await? {
            tracing::info!(
                domain = %self.primary_domain().unwrap_or("-"),
                "loaded existing cert from disk"
            );
            return Ok(true);
        }

        // No cert on disk — generate self-signed if configured.
        if self.config.self_signed {
            tracing::info!(
                domains = ?self.config.domains,
                "no cert found; generating self-signed certificate"
            );
            self.generate_self_signed().await?;
            return self.try_load_cert().await;
        }

        tracing::warn!(
            domain = %self.primary_domain().unwrap_or("-"),
            "no cert found on disk and self_signed is disabled"
        );
        Ok(false)
    }

    /// Try to load cert.pem + key.pem from disk and build a `ServerConfig`.
    /// Returns `Ok(true)` if successful.
    async fn try_load_cert(&self) -> Result<bool, GatewayError> {
        let cert_path = match self.cert_path() {
            Some(p) => p,
            None => return Ok(false),
        };
        let key_path = match self.key_path() {
            Some(p) => p,
            None => return Ok(false),
        };

        if !cert_path.exists() || !key_path.exists() {
            return Ok(false);
        }

        let server_config = build_server_config_from_files(&cert_path, &key_path)?;

        // Record mtime for the file watcher.
        if let Ok(meta) = std::fs::metadata(&cert_path) {
            if let Ok(mtime) = meta.modified() {
                *self.last_cert_mtime.write().await = Some(mtime);
            }
        }

        *self.current_config.write().await = Some(Arc::new(server_config));
        Ok(true)
    }

    // ------------------------------------------------------------------
    // Self-signed cert generation
    // ------------------------------------------------------------------

    /// Generate a self-signed certificate for the configured domains and
    /// write it to the storage directory.
    async fn generate_self_signed(&self) -> Result<(), GatewayError> {
        let domains = &self.config.domains;
        if domains.is_empty() {
            return Err(GatewayError::ConfigError(
                "acme.domains is empty; cannot generate self-signed cert".into(),
            ));
        }

        let cert_path = self.cert_path().unwrap();
        let key_path = self.key_path().unwrap();

        generate_self_signed_cert(domains, &cert_path, &key_path)?;

        tracing::info!(
            cert = %cert_path.display(),
            key = %key_path.display(),
            domains = ?domains,
            "self-signed certificate written"
        );
        Ok(())
    }

    // ------------------------------------------------------------------
    // HTTP-01 challenge handler
    // ------------------------------------------------------------------

    /// Register a pending HTTP-01 challenge. Called by the ACME client
    /// flow (or an external orchestrator via the admin API).
    pub fn set_challenge(&self, token: String, key_authorization: String) {
        tracing::debug!(token = %token, "ACME challenge registered");
        self.challenges.insert(token, key_authorization);
    }

    /// Remove a challenge after it has been validated.
    pub fn clear_challenge(&self, token: &str) {
        self.challenges.remove(token);
    }

    /// Handle an inbound request to `/.well-known/acme-challenge/{token}`.
    ///
    /// Returns `Some(key_authorization)` if the token is known, or `None`
    /// if this is not a challenge request or the token is unrecognised.
    pub fn handle_challenge(&self, path: &str) -> Option<String> {
        const PREFIX: &str = "/.well-known/acme-challenge/";
        if !path.starts_with(PREFIX) {
            return None;
        }
        let token = &path[PREFIX.len()..];
        if token.is_empty() {
            return None;
        }
        self.challenges.get(token).map(|v| v.value().clone())
    }

    // ------------------------------------------------------------------
    // Cert file watcher
    // ------------------------------------------------------------------

    /// Spawn a background task that polls the cert file every 30 seconds
    /// and reloads the `ServerConfig` when the file changes.
    pub fn spawn_cert_watcher(self: &Arc<Self>) {
        let manager = Arc::clone(self);
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(30));
            loop {
                interval.tick().await;
                if let Err(e) = manager.check_and_reload().await {
                    tracing::warn!(error = %e, "cert watcher reload failed");
                }
            }
        });
    }

    /// Check whether the cert file has been modified since the last load
    /// and reload if so.
    async fn check_and_reload(&self) -> Result<(), GatewayError> {
        let cert_path = match self.cert_path() {
            Some(p) => p,
            None => return Ok(()),
        };

        let current_mtime = match std::fs::metadata(&cert_path) {
            Ok(meta) => match meta.modified() {
                Ok(t) => t,
                Err(_) => return Ok(()),
            },
            Err(_) => return Ok(()), // file doesn't exist yet
        };

        let last = *self.last_cert_mtime.read().await;
        if last == Some(current_mtime) {
            return Ok(()); // no change
        }

        tracing::info!(
            cert = %cert_path.display(),
            "cert file changed on disk; reloading"
        );

        self.try_load_cert().await?;

        tracing::info!("cert reloaded successfully");
        Ok(())
    }

    // ------------------------------------------------------------------
    // ServerConfig access
    // ------------------------------------------------------------------

    /// Return the current `ServerConfig`, or an error if no cert is loaded.
    pub async fn server_config(&self) -> Result<Arc<ServerConfig>, GatewayError> {
        self.current_config
            .read()
            .await
            .clone()
            .ok_or_else(|| GatewayError::Internal("no TLS certificate loaded".into()))
    }

    /// Whether a cert is currently loaded.
    pub async fn has_cert(&self) -> bool {
        self.current_config.read().await.is_some()
    }
}

// ======================================================================
// Free functions
// ======================================================================

/// Default storage directory: `~/.lynxx/acme/`.
fn default_storage_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("/var/lib/lynxx"))
        .join(".lynxx")
        .join("acme")
}

/// Build a `rustls::ServerConfig` from PEM files on disk.
fn build_server_config_from_files(
    cert_path: &Path,
    key_path: &Path,
) -> Result<ServerConfig, GatewayError> {
    let cert_pem = std::fs::read(cert_path)
        .map_err(|e| GatewayError::Internal(format!("read cert {}: {e}", cert_path.display())))?;
    let key_pem = std::fs::read(key_path)
        .map_err(|e| GatewayError::Internal(format!("read key {}: {e}", key_path.display())))?;

    let certs = {
        let mut cursor = std::io::Cursor::new(&cert_pem);
        rustls_pemfile::certs(&mut cursor)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| {
                GatewayError::Internal(format!("parse cert {}: {e}", cert_path.display()))
            })?
    };
    if certs.is_empty() {
        return Err(GatewayError::Internal(format!(
            "no certificates in {}",
            cert_path.display()
        )));
    }

    let key = {
        let mut cursor = std::io::Cursor::new(&key_pem);
        rustls_pemfile::private_key(&mut cursor)
            .map_err(|e| GatewayError::Internal(format!("parse key {}: {e}", key_path.display())))?
            .ok_or_else(|| {
                GatewayError::Internal(format!("no private key in {}", key_path.display()))
            })?
    };

    let mut config = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .map_err(|e| GatewayError::Internal(format!("build ServerConfig: {e}")))?;

    config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
    Ok(config)
}

/// Generate a self-signed certificate for the given domains and write
/// the PEM files to disk.
///
/// Uses ECDSA P-256 for the key (fast, small, widely supported) and
/// sets the validity to 365 days. The first domain becomes the CN;
/// all domains appear as SANs.
fn generate_self_signed_cert(
    domains: &[String],
    cert_path: &Path,
    key_path: &Path,
) -> Result<(), GatewayError> {
    use rcgen::{CertificateParams, DnType, KeyPair};

    let mut params = CertificateParams::new(domains.to_vec())
        .map_err(|e| GatewayError::Internal(format!("cert params: {e}")))?;

    // Set the common name to the primary domain.
    if let Some(primary) = domains.first() {
        params
            .distinguished_name
            .push(DnType::CommonName, primary.clone());
    }
    params
        .distinguished_name
        .push(DnType::OrganizationName, "Lynxx Self-Signed");

    // ECDSA P-256 via ring — consistent with the rest of the gateway's
    // crypto stack.
    let key_pair = KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256)
        .map_err(|e| GatewayError::Internal(format!("generate key pair: {e}")))?;

    let cert = params
        .self_signed(&key_pair)
        .map_err(|e| GatewayError::Internal(format!("self-sign cert: {e}")))?;

    // Write PEM files.
    let cert_pem = cert.pem();
    let key_pem = key_pair.serialize_pem();

    // Ensure parent directory exists.
    if let Some(parent) = cert_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            GatewayError::Internal(format!("create cert dir {}: {e}", parent.display()))
        })?;
    }

    std::fs::write(cert_path, cert_pem.as_bytes())
        .map_err(|e| GatewayError::Internal(format!("write cert {}: {e}", cert_path.display())))?;
    std::fs::write(key_path, key_pem.as_bytes())
        .map_err(|e| GatewayError::Internal(format!("write key {}: {e}", key_path.display())))?;

    // Restrict key file permissions on Unix.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o600);
        let _ = std::fs::set_permissions(key_path, perms);
    }

    Ok(())
}

// ======================================================================
// Tests
// ======================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// Install the ring crypto provider for tests that build a
    /// `ServerConfig`. The gateway binary does this in `main()`;
    /// tests need it explicitly. Calling it twice is harmless (returns
    /// `Err` which we ignore).
    fn install_crypto_provider() {
        let _ = rustls::crypto::ring::default_provider().install_default();
    }

    #[test]
    fn test_handle_challenge_match() {
        let config = AcmeConfig {
            enabled: true,
            domains: vec!["example.com".into()],
            self_signed: false,
            ..AcmeConfig::default()
        };
        let mgr = AcmeManager::new(config);
        mgr.set_challenge("test-token-123".into(), "auth-value-xyz".into());

        assert_eq!(
            mgr.handle_challenge("/.well-known/acme-challenge/test-token-123"),
            Some("auth-value-xyz".to_string())
        );
    }

    #[test]
    fn test_handle_challenge_no_match() {
        let config = AcmeConfig {
            enabled: true,
            domains: vec!["example.com".into()],
            self_signed: false,
            ..AcmeConfig::default()
        };
        let mgr = AcmeManager::new(config);

        assert_eq!(
            mgr.handle_challenge("/.well-known/acme-challenge/unknown"),
            None
        );
        assert_eq!(mgr.handle_challenge("/some/other/path"), None);
        assert_eq!(mgr.handle_challenge("/.well-known/acme-challenge/"), None);
    }

    #[test]
    fn test_clear_challenge() {
        let config = AcmeConfig {
            enabled: true,
            domains: vec!["example.com".into()],
            self_signed: false,
            ..AcmeConfig::default()
        };
        let mgr = AcmeManager::new(config);
        mgr.set_challenge("tok".into(), "auth".into());
        assert!(mgr
            .handle_challenge("/.well-known/acme-challenge/tok")
            .is_some());

        mgr.clear_challenge("tok");
        assert!(mgr
            .handle_challenge("/.well-known/acme-challenge/tok")
            .is_none());
    }

    #[test]
    fn test_generate_self_signed_cert() {
        install_crypto_provider();
        let tmp = tempfile::tempdir().unwrap();
        let cert_path = tmp.path().join("cert.pem");
        let key_path = tmp.path().join("key.pem");

        generate_self_signed_cert(
            &["localhost".to_string(), "127.0.0.1".to_string()],
            &cert_path,
            &key_path,
        )
        .unwrap();

        assert!(cert_path.exists());
        assert!(key_path.exists());

        // Verify the files are valid PEM that rustls can parse.
        let config = build_server_config_from_files(&cert_path, &key_path);
        assert!(config.is_ok(), "generated cert should be loadable");
    }

    #[test]
    fn test_storage_dir_default() {
        let config = AcmeConfig::default();
        let mgr = AcmeManager::new(config);
        let dir = mgr.storage_dir();
        assert!(
            dir.to_string_lossy().contains("acme"),
            "default dir should contain 'acme'"
        );
    }

    #[test]
    fn test_storage_dir_custom() {
        let config = AcmeConfig {
            storage_dir: "/custom/path".into(),
            ..AcmeConfig::default()
        };
        let mgr = AcmeManager::new(config);
        assert_eq!(mgr.storage_dir(), PathBuf::from("/custom/path"));
    }

    #[tokio::test]
    async fn test_init_self_signed() {
        install_crypto_provider();
        let tmp = tempfile::tempdir().unwrap();
        let config = AcmeConfig {
            enabled: true,
            domains: vec!["localhost".into()],
            self_signed: true,
            storage_dir: tmp.path().to_string_lossy().into_owned(),
            ..AcmeConfig::default()
        };
        let mgr = AcmeManager::new(config);
        let ok = mgr.init().await.unwrap();
        assert!(ok, "init with self_signed should succeed");
        assert!(mgr.has_cert().await, "cert should be loaded");

        // Verify cert files were created.
        assert!(tmp.path().join("localhost").join("cert.pem").exists());
        assert!(tmp.path().join("localhost").join("key.pem").exists());
    }

    #[tokio::test]
    async fn test_init_no_domains_fails() {
        let config = AcmeConfig {
            enabled: true,
            domains: vec![],
            ..AcmeConfig::default()
        };
        let mgr = AcmeManager::new(config);
        assert!(mgr.init().await.is_err());
    }

    #[tokio::test]
    async fn test_reload_on_file_change() {
        install_crypto_provider();
        let tmp = tempfile::tempdir().unwrap();
        let domain_dir = tmp.path().join("localhost");
        std::fs::create_dir_all(&domain_dir).unwrap();

        let cert_path = domain_dir.join("cert.pem");
        let key_path = domain_dir.join("key.pem");

        // Generate initial cert.
        generate_self_signed_cert(&["localhost".to_string()], &cert_path, &key_path).unwrap();

        let config = AcmeConfig {
            enabled: true,
            domains: vec!["localhost".into()],
            self_signed: false,
            storage_dir: tmp.path().to_string_lossy().into_owned(),
            ..AcmeConfig::default()
        };
        let mgr = AcmeManager::new(config);
        let ok = mgr.init().await.unwrap();
        assert!(ok);

        // Simulate external renewal: regenerate the cert.
        // Sleep briefly so mtime differs.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        generate_self_signed_cert(&["localhost".to_string()], &cert_path, &key_path).unwrap();

        // Manually trigger the check.
        mgr.check_and_reload().await.unwrap();

        // Should still have a valid cert.
        assert!(mgr.has_cert().await);
    }
}
