//! Certificate manager with ACME HTTP-01 + TLS-ALPN-01 challenge
//! support, cert file watching, and self-signed certificate generation.
//!
//! This module provides four practical capabilities for TLS provisioning:
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
//! 3. **TLS-ALPN-01 challenge responder** (RFC 8737): When the
//!    validator opens a TLS connection on port 443 with the
//!    `acme-tls/1` ALPN protocol, the gateway presents a short-lived
//!    challenge cert that embeds SHA-256(key authorization) in the
//!    `id-pe-acmeIdentifier` extension. This means operators who have
//!    port 80 closed (only 443 reachable) can still complete ACME
//!    validation. See [`crate::proxy::tls_alpn`] for the implementation.
//!
//! 4. **Cert file watcher**: Polls cert files on disk every 30 seconds
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
use crate::proxy::tls_alpn::{
    build_challenge_cert, certified_key_from_pem, AcmeChallengeResolver, ACME_TLS_ALPN_NAME,
};
use dashmap::DashMap;
use rustls::sign::CertifiedKey;
use rustls::ServerConfig;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;
use tokio::sync::RwLock;

/// Manages certificate lifecycle: loading, generation, watching, and
/// ACME HTTP-01 + TLS-ALPN-01 challenge handling.
pub struct AcmeManager {
    config: AcmeConfig,
    /// Current rustls `ServerConfig`, swapped atomically on reload.
    current_config: Arc<RwLock<Option<Arc<ServerConfig>>>>,
    /// Pending HTTP-01 challenges: token -> key-authorization.
    challenges: Arc<DashMap<String, String>>,
    /// Pending TLS-ALPN-01 challenges: domain -> challenge cert. The
    /// resolver inside the active `ServerConfig` queries this map on
    /// every ClientHello whose ALPN includes `acme-tls/1`.
    tls_alpn_challenges: Arc<DashMap<String, Arc<CertifiedKey>>>,
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
            tls_alpn_challenges: Arc::new(DashMap::new()),
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
    ///
    /// The built `ServerConfig` uses [`AcmeChallengeResolver`] so the
    /// same listener can serve production traffic *and* TLS-ALPN-01
    /// challenge certs without any restart — the resolver picks based
    /// on the ALPN protocol negotiated on the inbound ClientHello.
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

        let cert_pem = std::fs::read(&cert_path).map_err(|e| {
            GatewayError::Internal(format!("read cert {}: {e}", cert_path.display()))
        })?;
        let key_pem = std::fs::read(&key_path)
            .map_err(|e| GatewayError::Internal(format!("read key {}: {e}", key_path.display())))?;
        let production_key = Arc::new(certified_key_from_pem(&cert_pem, &key_pem)?);

        let lookup_map = Arc::clone(&self.tls_alpn_challenges);
        let resolver = AcmeChallengeResolver::new(production_key, move |sni| {
            lookup_map.get(sni).map(|v| Arc::clone(v.value()))
        });

        let mut server_config = ServerConfig::builder()
            .with_no_client_auth()
            .with_cert_resolver(Arc::new(resolver));

        // Order matters: the validator advertises *only* `acme-tls/1`
        // for challenges, while production clients negotiate `h2` /
        // `http/1.1`. Including `acme-tls/1` here makes rustls accept
        // it during the ALPN check. The resolver picks the right cert.
        server_config.alpn_protocols = vec![
            b"h2".to_vec(),
            b"http/1.1".to_vec(),
            ACME_TLS_ALPN_NAME.to_vec(),
        ];

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
    // TLS-ALPN-01 challenge handler
    // ------------------------------------------------------------------

    /// Register a TLS-ALPN-01 challenge for `domain`. Builds a
    /// short-lived self-signed certificate that embeds
    /// SHA-256(`key_authorization`) in the `id-pe-acmeIdentifier`
    /// extension, and stores it so the inbound resolver can present it
    /// when the validator opens an `acme-tls/1` connection.
    ///
    /// Same shape as [`set_challenge`] — used the same way by an
    /// external ACME orchestrator or the admin API.
    pub fn set_tls_alpn_challenge(
        &self,
        domain: String,
        key_authorization: String,
    ) -> Result<(), GatewayError> {
        let cert = build_challenge_cert(&domain, &key_authorization)?;
        tracing::debug!(domain = %domain, "TLS-ALPN-01 challenge registered");
        self.tls_alpn_challenges.insert(domain, Arc::new(cert));
        Ok(())
    }

    /// Remove a TLS-ALPN-01 challenge after the validator finishes.
    pub fn clear_tls_alpn_challenge(&self, domain: &str) {
        self.tls_alpn_challenges.remove(domain);
    }

    /// Number of TLS-ALPN-01 challenges currently registered. Cheap;
    /// used by the admin API for visibility.
    pub fn tls_alpn_challenge_count(&self) -> usize {
        self.tls_alpn_challenges.len()
    }

    /// Number of HTTP-01 challenges currently registered. Mirror of
    /// [`tls_alpn_challenge_count`]; used by `GET /admin/v1/acme/status`.
    pub fn http01_challenge_count(&self) -> usize {
        self.challenges.len()
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

    /// Return the special TLS-ALPN-01 challenge `ServerConfig` for the
    /// given SNI domain, if one is currently registered. Callers use
    /// this to intercept the TLS handshake during ACME validation.
    pub fn alpn_challenge_config(&self, server_name: &str) -> Option<Arc<ServerConfig>> {
        let ck = self.tls_alpn_challenges.get(server_name)?;
        let certified_key = Arc::clone(ck.value());
        let resolver = AlpnChallengeStaticResolver(certified_key);
        let mut cfg = ServerConfig::builder()
            .with_no_client_auth()
            .with_cert_resolver(Arc::new(resolver));
        cfg.alpn_protocols = vec![ACME_TLS_ALPN_NAME.to_vec()];
        Some(Arc::new(cfg))
    }

    /// Read the `AcmeConfig` (immutable after construction).
    pub fn config(&self) -> &AcmeConfig {
        &self.config
    }

    // ------------------------------------------------------------------
    // ACME account management
    // ------------------------------------------------------------------

    /// Path to the persisted ACME account credentials.
    fn account_path(&self) -> PathBuf {
        self.storage_dir().join("account.json")
    }

    /// Load an existing ACME account from disk, or create a new one at
    /// the configured directory URL and persist the credentials.
    async fn get_or_create_account(&self) -> Result<instant_acme::Account, GatewayError> {
        let account_path = self.account_path();
        let dir_url = &self.config.directory_url;

        if account_path.exists() {
            tracing::info!(path = %account_path.display(), "loading existing ACME account");
            let data = std::fs::read(&account_path).map_err(|e| {
                GatewayError::Internal(format!("read account {}: {e}", account_path.display()))
            })?;
            let creds: instant_acme::AccountCredentials = serde_json::from_slice(&data)
                .map_err(|e| GatewayError::Internal(format!("parse account.json: {e}")))?;
            let account = instant_acme::Account::from_credentials(creds)
                .await
                .map_err(|e| GatewayError::Internal(format!("restore ACME account: {e}")))?;
            tracing::info!("ACME account restored from disk");
            return Ok(account);
        }

        // Create a new account.
        let contact = if self.config.email.is_empty() {
            vec![]
        } else {
            vec![format!("mailto:{}", self.config.email)]
        };
        let contact_refs: Vec<&str> = contact.iter().map(|s| s.as_str()).collect();

        tracing::info!(
            directory = %dir_url,
            email = %self.config.email,
            "creating new ACME account"
        );

        let (account, credentials) = instant_acme::Account::create(
            &instant_acme::NewAccount {
                contact: &contact_refs,
                terms_of_service_agreed: true,
                only_return_existing: false,
            },
            dir_url,
            None,
        )
        .await
        .map_err(|e| GatewayError::Internal(format!("create ACME account: {e}")))?;

        // Persist to disk.
        let json = serde_json::to_vec_pretty(&credentials)
            .map_err(|e| GatewayError::Internal(format!("serialize account credentials: {e}")))?;

        if let Some(parent) = account_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                GatewayError::Internal(format!("create account dir {}: {e}", parent.display()))
            })?;
        }
        std::fs::write(&account_path, &json).map_err(|e| {
            GatewayError::Internal(format!("write account {}: {e}", account_path.display()))
        })?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&account_path, std::fs::Permissions::from_mode(0o600));
        }

        tracing::info!(
            id = %account.id(),
            path = %account_path.display(),
            "ACME account created and saved"
        );
        Ok(account)
    }

    // ------------------------------------------------------------------
    // Full ACME order flow
    // ------------------------------------------------------------------

    /// Request a certificate from the configured ACME directory for all
    /// configured domains. Handles:
    ///
    /// 1. Account creation / loading
    /// 2. Order creation
    /// 3. Authorization challenges (HTTP-01 or TLS-ALPN-01 based on
    ///    `challenge_type` config)
    /// 4. Order finalization with a fresh CSR
    /// 5. Certificate download and disk persistence
    /// 6. Hot-reload of the `ServerConfig`
    pub async fn request_certificate(&self) -> Result<(), GatewayError> {
        let domains = &self.config.domains;
        if domains.is_empty() {
            return Err(GatewayError::ConfigError(
                "acme.domains must contain at least one domain".into(),
            ));
        }

        tracing::info!(domains = ?domains, "starting ACME certificate request");

        // Step 1: account.
        let account = self.get_or_create_account().await?;

        // Step 2: create order.
        let identifiers: Vec<instant_acme::Identifier> = domains
            .iter()
            .map(|d| instant_acme::Identifier::Dns(d.clone()))
            .collect();
        let mut order = account
            .new_order(&instant_acme::NewOrder {
                identifiers: &identifiers,
            })
            .await
            .map_err(|e| GatewayError::Internal(format!("create ACME order: {e}")))?;

        tracing::info!(status = ?order.state().status, "ACME order created");

        // Step 3: handle authorizations.
        let authorizations = order
            .authorizations()
            .await
            .map_err(|e| GatewayError::Internal(format!("fetch authorizations: {e}")))?;

        let challenge_type = self.config.challenge_type.as_str();

        for authz in &authorizations {
            if authz.status == instant_acme::AuthorizationStatus::Valid {
                tracing::info!(
                    identifier = ?authz.identifier,
                    "authorization already valid, skipping"
                );
                continue;
            }

            let instant_acme::Identifier::Dns(ref domain) = authz.identifier;

            // Pick the right challenge based on config.
            let challenge = self
                .pick_challenge(&authz.challenges, challenge_type)
                .ok_or_else(|| {
                    GatewayError::Internal(format!(
                        "no suitable challenge for domain {domain} with type '{challenge_type}'"
                    ))
                })?;

            let key_auth = order.key_authorization(challenge);

            match challenge.r#type {
                instant_acme::ChallengeType::Http01 => {
                    tracing::info!(
                        domain = %domain,
                        token = %challenge.token,
                        "setting up HTTP-01 challenge"
                    );
                    self.set_challenge(challenge.token.clone(), key_auth.as_str().to_string());
                }
                instant_acme::ChallengeType::TlsAlpn01 => {
                    tracing::info!(domain = %domain, "setting up TLS-ALPN-01 challenge");
                    self.set_tls_alpn_challenge(domain.clone(), key_auth.as_str().to_string())?;
                }
                _ => {
                    return Err(GatewayError::Internal(format!(
                        "unsupported challenge type {:?}",
                        challenge.r#type
                    )));
                }
            }

            // Tell the server the challenge is ready.
            order
                .set_challenge_ready(&challenge.url)
                .await
                .map_err(|e| {
                    GatewayError::Internal(format!("set challenge ready for {domain}: {e}"))
                })?;

            tracing::info!(domain = %domain, "challenge marked ready");
        }

        // Step 4: poll until the order is ready or invalid.
        let mut delay = std::time::Duration::from_secs(1);
        for attempt in 1u8..=10 {
            tokio::time::sleep(delay).await;
            let state = order
                .refresh()
                .await
                .map_err(|e| GatewayError::Internal(format!("refresh order: {e}")))?;
            tracing::info!(attempt, status = ?state.status, "polling ACME order status");
            match state.status {
                instant_acme::OrderStatus::Ready | instant_acme::OrderStatus::Valid => break,
                instant_acme::OrderStatus::Invalid => {
                    self.cleanup_challenges(&authorizations);
                    return Err(GatewayError::Internal(format!(
                        "ACME order became invalid: {:?}",
                        state.error
                    )));
                }
                _ => {
                    delay = std::cmp::min(delay * 2, std::time::Duration::from_secs(30));
                }
            }
        }

        // Clean up challenges now that validation is done.
        self.cleanup_challenges(&authorizations);

        if order.state().status == instant_acme::OrderStatus::Ready {
            // Step 5: finalize with a CSR.
            tracing::info!("generating CSR and finalizing order");

            let mut csr_params = rcgen::CertificateParams::new(domains.to_vec())
                .map_err(|e| GatewayError::Internal(format!("CSR params: {e}")))?;
            csr_params.distinguished_name = rcgen::DistinguishedName::new();

            let private_key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256)
                .map_err(|e| GatewayError::Internal(format!("generate CSR key: {e}")))?;

            let csr = csr_params
                .serialize_request(&private_key)
                .map_err(|e| GatewayError::Internal(format!("serialize CSR: {e}")))?;

            order
                .finalize(csr.der())
                .await
                .map_err(|e| GatewayError::Internal(format!("finalize order: {e}")))?;

            // Step 6: poll for the certificate.
            let cert_pem = {
                let mut pem = None;
                for _ in 0..10 {
                    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                    match order.certificate().await {
                        Ok(Some(chain)) => {
                            pem = Some(chain);
                            break;
                        }
                        Ok(None) => {
                            tracing::info!("certificate not ready yet, polling...");
                        }
                        Err(e) => {
                            return Err(GatewayError::Internal(format!(
                                "download certificate: {e}"
                            )));
                        }
                    }
                }
                pem.ok_or_else(|| {
                    GatewayError::Internal("timed out waiting for certificate".into())
                })?
            };

            // Step 7: write cert + key to disk.
            let cert_path = self
                .cert_path()
                .ok_or_else(|| GatewayError::Internal("no primary domain for cert path".into()))?;
            let key_path = self
                .key_path()
                .ok_or_else(|| GatewayError::Internal("no primary domain for key path".into()))?;

            if let Some(parent) = cert_path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| {
                    GatewayError::Internal(format!("create cert dir {}: {e}", parent.display()))
                })?;
            }

            let key_pem = private_key.serialize_pem();

            std::fs::write(&cert_path, cert_pem.as_bytes()).map_err(|e| {
                GatewayError::Internal(format!("write cert {}: {e}", cert_path.display()))
            })?;
            std::fs::write(&key_path, key_pem.as_bytes()).map_err(|e| {
                GatewayError::Internal(format!("write key {}: {e}", key_path.display()))
            })?;

            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o600));
            }

            tracing::info!(
                cert = %cert_path.display(),
                key = %key_path.display(),
                "ACME certificate written to disk"
            );
        }

        // Step 8: reload into the running server.
        self.try_load_cert().await?;
        tracing::info!("ACME certificate loaded into server");

        Ok(())
    }

    /// Pick a challenge from the list based on the configured type.
    ///
    /// * `"http-01"` -- only HTTP-01
    /// * `"tls-alpn-01"` -- only TLS-ALPN-01
    /// * `"auto"` -- prefer TLS-ALPN-01, fall back to HTTP-01
    fn pick_challenge<'a>(
        &self,
        challenges: &'a [instant_acme::Challenge],
        challenge_type: &str,
    ) -> Option<&'a instant_acme::Challenge> {
        match challenge_type {
            "http-01" => challenges
                .iter()
                .find(|c| c.r#type == instant_acme::ChallengeType::Http01),
            "tls-alpn-01" => challenges
                .iter()
                .find(|c| c.r#type == instant_acme::ChallengeType::TlsAlpn01),
            _ => {
                // "auto": prefer TLS-ALPN-01, fall back to HTTP-01.
                challenges
                    .iter()
                    .find(|c| c.r#type == instant_acme::ChallengeType::TlsAlpn01)
                    .or_else(|| {
                        challenges
                            .iter()
                            .find(|c| c.r#type == instant_acme::ChallengeType::Http01)
                    })
            }
        }
    }

    /// Remove all pending challenges registered during this order.
    fn cleanup_challenges(&self, authorizations: &[instant_acme::Authorization]) {
        for authz in authorizations {
            let instant_acme::Identifier::Dns(ref domain) = authz.identifier;
            for ch in &authz.challenges {
                match ch.r#type {
                    instant_acme::ChallengeType::Http01 => {
                        self.clear_challenge(&ch.token);
                    }
                    instant_acme::ChallengeType::TlsAlpn01 => {
                        self.clear_tls_alpn_challenge(domain);
                    }
                    _ => {}
                }
            }
        }
    }

    // ------------------------------------------------------------------
    // Auto-renewal
    // ------------------------------------------------------------------

    /// Spawn a background task that checks cert expiry every 12 hours
    /// and triggers a new ACME order if the certificate is within 30
    /// days of expiry (or missing entirely).
    pub fn spawn_auto_renew(self: &Arc<Self>) {
        let manager = Arc::clone(self);
        tokio::spawn(async move {
            // Short initial delay to let the server finish booting.
            tokio::time::sleep(std::time::Duration::from_secs(10)).await;
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(12 * 3600));
            loop {
                interval.tick().await;
                if let Err(e) = manager.check_and_renew().await {
                    tracing::warn!(error = %e, "auto-renew check failed");
                }
            }
        });
    }

    /// Check whether the current cert needs renewal and, if so, run
    /// the full ACME order flow.
    async fn check_and_renew(&self) -> Result<(), GatewayError> {
        let cert_path = match self.cert_path() {
            Some(p) if p.exists() => p,
            _ => {
                tracing::info!("no cert on disk; requesting initial certificate");
                return self.request_certificate().await;
            }
        };

        // Parse the leaf certificate to find the expiry.
        let pem_bytes = std::fs::read(&cert_path).map_err(|e| {
            GatewayError::Internal(format!(
                "read cert for renewal check {}: {e}",
                cert_path.display()
            ))
        })?;

        let mut cursor = std::io::Cursor::new(&pem_bytes);
        let certs: Vec<rustls::pki_types::CertificateDer<'static>> =
            rustls_pemfile::certs(&mut cursor)
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| GatewayError::Internal(format!("parse cert for renewal: {e}")))?;

        if certs.is_empty() {
            tracing::info!("cert file empty; requesting new certificate");
            return self.request_certificate().await;
        }

        let (_, parsed) = x509_parser::parse_x509_certificate(certs[0].as_ref())
            .map_err(|e| GatewayError::Internal(format!("parse x509 for expiry: {e}")))?;

        // Treat the self-signed boot cert as immediately needing replacement.
        // generate_self_signed_cert sets the issuer's Organization to
        // "Lynxx Self-Signed" — that string is the cleanest sentinel since
        // a real LE cert's issuer is "Let's Encrypt R3"/etc.
        let issuer = parsed.issuer().to_string();
        if issuer.contains("Lynxx Self-Signed") {
            tracing::info!(issuer = %issuer, "self-signed boot cert detected; requesting real ACME certificate");
            return self.request_certificate().await;
        }

        let not_after = parsed.validity().not_after.timestamp();
        let now = chrono::Utc::now().timestamp();
        let days_remaining = (not_after - now) / 86400;

        tracing::info!(
            days_remaining,
            not_after = %parsed.validity().not_after,
            "cert expiry check"
        );

        if days_remaining <= 30 {
            tracing::info!(
                days_remaining,
                "cert expires within 30 days; requesting renewal"
            );
            self.request_certificate().await
        } else {
            tracing::info!(days_remaining, "cert is still valid, no renewal needed");
            Ok(())
        }
    }
}

// ======================================================================
// Free functions / helpers
// ======================================================================

/// Trivial cert resolver that always returns the same [`CertifiedKey`].
/// Used by [`AcmeManager::alpn_challenge_config`] to build a one-shot
/// `ServerConfig` for TLS-ALPN-01 validation.
#[derive(Debug)]
struct AlpnChallengeStaticResolver(Arc<CertifiedKey>);

impl rustls::server::ResolvesServerCert for AlpnChallengeStaticResolver {
    fn resolve(&self, _hello: rustls::server::ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        Some(Arc::clone(&self.0))
    }
}

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

    #[tokio::test]
    async fn test_tls_alpn_challenge_register_and_clear() {
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
        mgr.init().await.unwrap();

        assert_eq!(mgr.tls_alpn_challenge_count(), 0);

        mgr.set_tls_alpn_challenge("api.example.com".into(), "tok.thumb".into())
            .unwrap();
        mgr.set_tls_alpn_challenge("admin.example.com".into(), "tok.thumb".into())
            .unwrap();
        assert_eq!(mgr.tls_alpn_challenge_count(), 2);

        // The resolver inside the ServerConfig should now also know
        // about the registered challenge — i.e. the lookup closure
        // returns Some when invoked with a registered SNI. We can't
        // easily synthesize a ClientHello in a unit test, so the
        // count check stands in for round-trip verification — the
        // ALPN <-> challenge flow itself is covered by
        // `tls_alpn::tests::builds_challenge_cert_with_acme_extension`
        // and the integration tests under tests/.
        mgr.clear_tls_alpn_challenge("api.example.com");
        assert_eq!(mgr.tls_alpn_challenge_count(), 1);
        mgr.clear_tls_alpn_challenge("admin.example.com");
        assert_eq!(mgr.tls_alpn_challenge_count(), 0);
    }
}
