//! Process-wide wasmtime engine + epoch ticker.
//!
//! We instantiate exactly one `wasmtime::Engine` for the entire process.
//! Engines are expensive (they own JIT caches, signal handlers, the
//! cranelift backend) but cheap to share across stores — one per request
//! is the cheap path.
//!
//! The epoch interruption mechanism is how we enforce wall-clock
//! deadlines: a background tokio task increments the engine's epoch
//! every millisecond, and each request configures
//! `store.set_epoch_deadline(N)` so the guest is interrupted after N
//! ticks. This is dramatically cheaper than spawning a watchdog thread
//! per request.

use crate::error::GatewayError;
use std::sync::Arc;
use std::sync::OnceLock;
use std::time::Duration;
use wasmtime::{Config, Engine};

/// Global, lazily-initialized wasmtime engine. Behind a `OnceLock` so
/// the first `WasmPlugin::from_config` call materializes it and every
/// subsequent plugin reuses the same JIT cache.
static ENGINE: OnceLock<WasmEngine> = OnceLock::new();

#[derive(Clone)]
pub struct WasmEngine {
    inner: Engine,
}

impl WasmEngine {
    pub fn global() -> Result<&'static WasmEngine, GatewayError> {
        if let Some(e) = ENGINE.get() {
            return Ok(e);
        }
        let engine = WasmEngine::build()?;
        // Best-effort init; if another thread won the race we just use theirs.
        let _ = ENGINE.set(engine);
        Ok(ENGINE.get().expect("engine initialized"))
    }

    fn build() -> Result<Self, GatewayError> {
        let mut cfg = Config::new();
        cfg.consume_fuel(true);
        cfg.epoch_interruption(true);
        // Cranelift on by default. We deliberately don't enable WASI —
        // the only host surface a guest sees is the `lynxx_*` ABI.
        let inner = Engine::new(&cfg)
            .map_err(|e| GatewayError::Internal(format!("wasmtime engine init failed: {e}")))?;
        let engine = WasmEngine { inner };
        engine.spawn_epoch_ticker();
        Ok(engine)
    }

    /// Background task: increment the engine epoch every 1ms. Survives
    /// the lifetime of the process. Anchored to the engine handle so the
    /// task naturally exits if the engine is ever dropped (today: never).
    fn spawn_epoch_ticker(&self) {
        let engine = self.inner.clone();
        // Only spawn the ticker when a tokio runtime is actually
        // available. This lets `cargo check` and pure-sync helpers
        // construct an engine without a runtime panic.
        let handle = match tokio::runtime::Handle::try_current() {
            Ok(h) => h,
            Err(_) => return,
        };
        handle.spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_millis(1));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                interval.tick().await;
                engine.increment_epoch();
            }
        });
    }

    pub fn engine(&self) -> &Engine {
        &self.inner
    }
}

/// Bytes-of-a-module helper: validate the wasm magic number before
/// handing the buffer to wasmtime. wasmtime would also reject malformed
/// bytes but this gives a clearer error to the dashboard.
pub fn validate_magic(bytes: &[u8]) -> Result<(), GatewayError> {
    if bytes.len() < 8 || bytes[0..4] != [0x00, 0x61, 0x73, 0x6d] {
        return Err(GatewayError::BadRequest(
            "module is not a valid WebAssembly binary (missing \\0asm magic)".into(),
        ));
    }
    Ok(())
}

/// Convenience wrapper used by tests to share the engine handle without
/// reaching through the `OnceLock` api.
pub fn shared_engine() -> Result<Arc<Engine>, GatewayError> {
    Ok(Arc::new(WasmEngine::global()?.engine().clone()))
}
