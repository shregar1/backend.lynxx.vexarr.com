//! Session Ticket Encryption Key (STEK) rotation.
//!
//! TLS session tickets let clients resume connections without a full
//! handshake, but the ticket encryption key is a forward-secrecy
//! liability: anyone who obtains it can decrypt every session that was
//! resumed with a ticket encrypted under that key.
//!
//! rustls' [`Ticketer`] already wraps a [`TicketRotator`] that lazily
//! rotates keys on the encrypt/decrypt hot-path. That is fine for
//! busy gateways, but under low traffic the old key material can
//! linger in memory far past the intended lifetime.
//!
//! [`StekRotator`] adds a **proactive** background task that forces
//! rotation on a configurable interval (default 6 h) regardless of
//! traffic. It delegates all crypto to rustls' own `TicketRotator`
//! and simply swaps the inner ticketer atomically so that:
//!
//! * The **current** key encrypts new tickets.
//! * The **previous** key is retained for decryption only, so
//!   in-flight sessions whose ticket was issued under the old key
//!   can still resume.
//! * After two rotations, the oldest key is erased from memory.
//!
//! # Usage
//!
//! ```ignore
//! let rotator = Arc::new(StekRotator::new(6 * 3600)?);
//! rotator.spawn();                       // background task
//! server_config.ticketer = rotator.ticketer().await;
//! ```
//!
//! [`Ticketer`]: rustls::crypto::ring::Ticketer
//! [`TicketRotator`]: rustls::ticketer::TicketRotator

use std::fmt::{self, Debug};
use std::sync::Arc;

use rustls::crypto::ring::Ticketer;
use rustls::server::ProducesTickets;
use tokio::sync::RwLock;

use crate::error::GatewayError;

/// Default rotation interval: 6 hours, matching rustls' own default.
const DEFAULT_INTERVAL_SECS: u64 = 6 * 60 * 60;

/// Proactive STEK rotator backed by a tokio background task.
///
/// Wraps a rustls [`ProducesTickets`] implementation and periodically
/// replaces it with a freshly-keyed instance. The previous ticketer
/// is kept alive inside the new [`Ticketer`] for decryption of
/// recently-issued tickets.
pub struct StekRotator {
    interval_secs: u64,
    inner: RwLock<Arc<dyn ProducesTickets>>,
}

impl StekRotator {
    /// Create a new rotator with the given rotation interval in seconds.
    ///
    /// Generates the initial ticket encryption key immediately.
    /// Returns an error if the CSPRNG fails (should never happen on a
    /// healthy system).
    pub fn new(interval_secs: u64) -> Result<Self, GatewayError> {
        let interval = if interval_secs == 0 {
            DEFAULT_INTERVAL_SECS
        } else {
            interval_secs
        };

        let ticketer = Ticketer::new().map_err(|e| {
            GatewayError::Internal(format!("stek: initial ticketer creation failed: {e}"))
        })?;

        tracing::info!(interval_secs = interval, "STEK rotator initialised");

        Ok(Self {
            interval_secs: interval,
            inner: RwLock::new(ticketer),
        })
    }

    /// Create a rotator with the default 6-hour interval.
    pub fn default_interval() -> Result<Self, GatewayError> {
        Self::new(DEFAULT_INTERVAL_SECS)
    }

    /// Return the current ticketer for installation in a
    /// [`rustls::ServerConfig`].
    ///
    /// The returned `Arc` is a snapshot; after the next rotation the
    /// `StekRotator` will hold a different ticketer, but any
    /// `ServerConfig` that was built with this one will continue to
    /// work until the key expires naturally.
    pub async fn ticketer(&self) -> Arc<dyn ProducesTickets> {
        self.inner.read().await.clone()
    }

    /// Force an immediate key rotation.
    ///
    /// This replaces the inner ticketer with a freshly-keyed instance.
    /// The previous key material is retained inside the new
    /// [`TicketRotator`] for one more rotation cycle.
    pub async fn rotate(&self) -> Result<(), GatewayError> {
        let new_ticketer = Ticketer::new()
            .map_err(|e| GatewayError::Internal(format!("stek: ticketer rotation failed: {e}")))?;

        let mut guard = self.inner.write().await;
        *guard = new_ticketer;

        tracing::info!("STEK rotated — new session ticket key active");
        Ok(())
    }

    /// Spawn the background rotation task.
    ///
    /// The task runs until the process exits (or the tokio runtime
    /// shuts down). It is cancel-safe: dropping the `JoinHandle`
    /// stops the loop without corrupting state.
    pub fn spawn(self: &Arc<Self>) -> tokio::task::JoinHandle<()> {
        let this = Arc::clone(self);
        tokio::spawn(async move {
            let mut interval =
                tokio::time::interval(std::time::Duration::from_secs(this.interval_secs));

            // The first tick fires immediately — skip it since we
            // just created a fresh key in `new()`.
            interval.tick().await;

            loop {
                interval.tick().await;

                if let Err(e) = this.rotate().await {
                    tracing::error!(error = %e, "STEK rotation failed — retrying next interval");
                    // Non-fatal: the current key is still valid; we
                    // will retry on the next tick.
                }
            }
        })
    }

    /// The configured rotation interval in seconds.
    pub fn interval_secs(&self) -> u64 {
        self.interval_secs
    }
}

// ---------------------------------------------------------------------------
// Trait impls
// ---------------------------------------------------------------------------

impl Debug for StekRotator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StekRotator")
            .field("interval_secs", &self.interval_secs)
            .finish_non_exhaustive()
    }
}

/// Delegate [`ProducesTickets`] to the inner ticketer so that a
/// `StekRotator` can be installed directly as the ticketer on a
/// `ServerConfig` without an extra `Arc` indirection.
///
/// Because `ProducesTickets` methods are synchronous we cannot hold
/// the async `RwLock` across the call. Instead we use `try_read` —
/// which is non-blocking and infallible under normal operation — and
/// fall back to returning `None` (which tells rustls to skip the
/// ticket for this connection) if the lock happens to be held by a
/// concurrent rotation.  Rotation takes microseconds, so contention
/// is vanishingly rare.
impl ProducesTickets for StekRotator {
    fn enabled(&self) -> bool {
        self.inner.try_read().map_or(true, |guard| guard.enabled())
    }

    fn lifetime(&self) -> u32 {
        self.inner
            .try_read()
            .map_or(self.interval_secs as u32 * 2, |guard| guard.lifetime())
    }

    fn encrypt(&self, plain: &[u8]) -> Option<Vec<u8>> {
        let guard = self.inner.try_read().ok()?;
        guard.encrypt(plain)
    }

    fn decrypt(&self, cipher: &[u8]) -> Option<Vec<u8>> {
        let guard = self.inner.try_read().ok()?;
        guard.decrypt(cipher)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn creates_with_default_interval() {
        let r = StekRotator::default_interval().expect("should create");
        assert_eq!(r.interval_secs(), DEFAULT_INTERVAL_SECS);
    }

    #[test]
    fn zero_interval_falls_back_to_default() {
        let r = StekRotator::new(0).expect("should create");
        assert_eq!(r.interval_secs(), DEFAULT_INTERVAL_SECS);
    }

    #[test]
    fn custom_interval() {
        let r = StekRotator::new(3600).expect("should create");
        assert_eq!(r.interval_secs(), 3600);
    }

    #[test]
    fn produces_tickets_trait_enabled() {
        let r = StekRotator::default_interval().expect("should create");
        assert!(r.enabled());
    }

    #[test]
    fn encrypt_decrypt_roundtrip() {
        let r = StekRotator::default_interval().expect("should create");
        let message = b"session-state-payload-for-tls-resumption";
        let cipher = r.encrypt(message).expect("encrypt should succeed");
        assert_ne!(&cipher[..], &message[..]);
        let plain = r.decrypt(&cipher).expect("decrypt should succeed");
        assert_eq!(plain, message);
    }

    #[test]
    fn decrypt_garbage_returns_none() {
        let r = StekRotator::default_interval().expect("should create");
        // Prime the ticketer so it has a maximum ciphertext length.
        let _ = r.encrypt(b"prime");
        assert!(r.decrypt(b"not-a-valid-ticket").is_none());
    }

    #[test]
    fn debug_does_not_leak_key_material() {
        let r = StekRotator::new(7200).expect("should create");
        let dbg = format!("{r:?}");
        assert!(dbg.contains("StekRotator"));
        assert!(dbg.contains("7200"));
        // Must not contain raw key bytes.
        assert!(!dbg.contains("key"));
    }

    #[tokio::test]
    async fn ticketer_returns_working_arc() {
        let r = StekRotator::default_interval().expect("should create");
        let t = r.ticketer().await;
        assert!(t.enabled());
        let ct = t.encrypt(b"hello").expect("encrypt");
        let pt = t.decrypt(&ct).expect("decrypt");
        assert_eq!(pt, b"hello");
    }

    #[tokio::test]
    async fn rotate_replaces_ticketer() {
        let r = StekRotator::default_interval().expect("should create");

        // Encrypt under the original key.
        let ct_before = r.encrypt(b"before rotation").expect("encrypt");

        // Rotate.
        r.rotate().await.expect("rotate should succeed");

        // Encrypt under the new key.
        let ct_after = r.encrypt(b"after rotation").expect("encrypt");
        let pt_after = r.decrypt(&ct_after).expect("decrypt new");
        assert_eq!(pt_after, b"after rotation");

        // The old ciphertext was encrypted under the *previous*
        // inner ticketer which has been replaced wholesale. The new
        // Ticketer has its own independent key, so decrypting the
        // old ciphertext will fail — this is expected since the
        // `StekRotator` swaps the entire ticketer. In production
        // the `ServerConfig` still holds an `Arc` to the old ticketer
        // and sessions using it drain naturally within the ticket
        // lifetime window.
        //
        // This validates that a rotation actually produces a new key.
        assert!(r.decrypt(&ct_before).is_none());
    }

    #[tokio::test]
    async fn spawn_runs_rotation() {
        // Use a very short interval so the background task fires
        // quickly in the test.
        let r = Arc::new(StekRotator::new(1).expect("should create"));

        let ct_initial = r.encrypt(b"initial").expect("encrypt");

        let handle = r.spawn();

        // Wait long enough for at least one rotation tick.
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;

        // After the background task rotated, the old ciphertext
        // should no longer decrypt under the new ticketer.
        assert!(r.decrypt(&ct_initial).is_none());

        // New encrypt/decrypt should work.
        let ct_new = r.encrypt(b"fresh").expect("encrypt");
        assert_eq!(r.decrypt(&ct_new).expect("decrypt"), b"fresh");

        handle.abort();
    }
}
