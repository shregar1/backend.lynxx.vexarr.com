use crate::error::GatewayError;
use serde::Deserialize;

/// `openapi-mock` plugin config blob.
///
/// Minimal — the spec path is the only required field. Everything else is
/// useful tuning for contract-test runs.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpenApiMockConfig {
    /// Filesystem path to the spec (YAML or JSON). We block at construction
    /// time to read it; the cost is paid once on plugin build.
    pub spec_path: String,
    /// Status to prefer when the matched operation declares multiple
    /// success responses. Defaults to 200.
    #[serde(default = "default_status")]
    pub default_status: u16,
    /// Artificial latency injected per response. Useful for testing
    /// client-side timeouts.
    #[serde(default)]
    pub delay_ms: u64,
    /// What to do if no operation matches (or the matched op has no
    /// example): true → pass the request through to upstream untouched,
    /// false → short-circuit with 501.
    #[serde(default = "default_passthrough")]
    pub passthrough_unmatched: bool,
}

fn default_status() -> u16 {
    200
}

fn default_passthrough() -> bool {
    true
}

/// Read the spec file synchronously. We're called from `from_config` which
/// runs on a Tokio worker but only at plugin-build time, so a blocking
/// `std::fs::read_to_string` is cheap and easier to reason about than a
/// dance through `tokio::fs`.
pub fn load_spec_blocking(path: &str) -> Result<String, GatewayError> {
    std::fs::read_to_string(path)
        .map_err(|e| GatewayError::BadRequest(format!("openapi-mock: cannot read {path}: {e}")))
}
