//! Outbound clients for Vexarr-internal services.
//!
//! Each submodule wraps one external HTTP API in a typed client. Keep
//! these crates dependency-light — they're called from boot paths and
//! background loops, not the proxy hot path, but they still share the
//! Tokio runtime and shouldn't drag in heavyweight runtime features.

pub mod keyx;
