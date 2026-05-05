//! Configuration types for the `llm-gateway` plugin.
//!
//! The plugin accepts a single JSON blob (see [`LlmGatewayConfig`]) describing
//! every provider, route, guard, cache, bucket, and price the runtime needs.
//! The shape is intentionally flat so it can be authored by hand without a
//! separate schema tool, and so the dashboard can render each section as its
//! own form group.
//!
//! All sub-structs implement `Default` so partial configs work — only
//! `providers` is mandatory at runtime.

use serde::Deserialize;
use std::collections::HashMap;

/// Top-level config consumed by [`crate::plugins::llm_gateway::LlmGatewayPlugin::from_config`].
///
/// Field names match the JSON schema documented in the plugin catalog so the
/// dashboard form can round-trip without a translation layer.
#[derive(Debug, Clone, Deserialize)]
pub struct LlmGatewayConfig {
    /// Upstream LLM providers known to the gateway. Order is significant:
    /// when no `model_routing` entry matches and no `default_provider` is set,
    /// the first entry wins.
    #[serde(default)]
    pub providers: Vec<ProviderCfg>,

    /// Provider name to use when `model_routing` does not match. If unset,
    /// falls back to `providers[0]`.
    #[serde(default)]
    pub default_provider: Option<String>,

    /// Map of model id -> provider name. Lookups are exact-match,
    /// case-sensitive.
    #[serde(default)]
    pub model_routing: HashMap<String, String>,

    /// Ordered chain of provider names to try if the primary returns 429 or
    /// 5xx. The primary is always first; entries already attempted are
    /// skipped on retry.
    #[serde(default)]
    pub fallback_chain: Vec<String>,

    #[serde(default)]
    pub rate_limit: RateLimitCfg,

    #[serde(default)]
    pub cache: CacheCfg,

    #[serde(default)]
    pub guard: GuardCfg,

    #[serde(default)]
    pub cost_tracking: CostCfg,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ProviderCfg {
    pub name: String,
    pub base_url: String,
    /// Provider-issued API key. Optional for local / offline providers
    /// (e.g. Ollama at `http://localhost:11434/v1`).
    #[serde(default)]
    pub api_key: Option<String>,
    /// HTTP header that carries the credential. OpenAI uses `Authorization`;
    /// Anthropic uses `x-api-key`.
    #[serde(default = "default_auth_header")]
    pub auth_header: String,
    /// Token scheme prefixed to the credential, with a single space separator.
    /// Empty string means the raw key is sent unprefixed (Anthropic shape).
    #[serde(default = "default_auth_scheme")]
    pub auth_scheme: String,
}

fn default_auth_header() -> String {
    "Authorization".to_string()
}

fn default_auth_scheme() -> String {
    "Bearer".to_string()
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct RateLimitCfg {
    #[serde(default)]
    pub enabled: bool,
    /// Per-consumer ceiling on the sum of input + output tokens within the
    /// rolling 60s window.
    #[serde(default = "default_tokens_per_minute")]
    pub tokens_per_minute_per_consumer: u64,
    /// Per-consumer ceiling on request count within the rolling 60s window.
    #[serde(default = "default_requests_per_minute")]
    pub requests_per_minute_per_consumer: u64,
}

fn default_tokens_per_minute() -> u64 {
    100_000
}

fn default_requests_per_minute() -> u64 {
    1_000
}

#[derive(Debug, Clone, Deserialize)]
pub struct CacheCfg {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_cache_ttl")]
    pub ttl_seconds: u64,
    /// v1 supports `"exact"` only. The field is kept so configs forward-compat
    /// with v2 (`"semantic"` via vector similarity).
    #[serde(default = "default_cache_strategy")]
    pub strategy: String,
}

impl Default for CacheCfg {
    fn default() -> Self {
        Self {
            enabled: false,
            ttl_seconds: default_cache_ttl(),
            strategy: default_cache_strategy(),
        }
    }
}

fn default_cache_ttl() -> u64 {
    3600
}

fn default_cache_strategy() -> String {
    "exact".to_string()
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct GuardCfg {
    #[serde(default)]
    pub block_prompt_injection: bool,
    /// Regex patterns. Compiled once at plugin construction.
    #[serde(default)]
    pub injection_patterns: Vec<String>,
    /// Names of PII categories to redact: `"email"`, `"ssn"`, `"phone"`,
    /// `"credit_card"`, `"ip"`. Anything else is ignored.
    #[serde(default)]
    pub redact_pii: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct CostCfg {
    #[serde(default)]
    pub enabled: bool,
    /// Per-million-tokens-but-quoted-per-thousand pricing table keyed by
    /// model id. Anything not in the map is treated as $0 — emit a metric
    /// label `model="unknown"` and move on.
    #[serde(default)]
    pub model_costs: HashMap<String, ModelCostCfg>,
}

#[derive(Debug, Clone, Copy, Deserialize, Default)]
pub struct ModelCostCfg {
    #[serde(default)]
    pub input_per_1k: f64,
    #[serde(default)]
    pub output_per_1k: f64,
}
