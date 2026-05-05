//! Model -> provider routing and the ordered fallback chain.
//!
//! The router is a pure data structure built once at plugin construction. It
//! holds:
//!
//! - the configured providers, indexed by name for O(1) lookup,
//! - the exact-match `model_routing` table,
//! - a `default_provider` used when nothing matches,
//! - the `fallback_chain` (order matters; duplicates filtered).
//!
//! Routing is exact-match by model id. We deliberately avoid prefix or regex
//! matching here — model ids change on a vendor-controlled cadence, and a
//! mistaken prefix match (e.g. `gpt-4` matching `gpt-4o`) would silently
//! route to the wrong upstream. Operators who want catch-all behaviour can
//! set `default_provider`.

use super::config::{LlmGatewayConfig, ProviderCfg};
use std::collections::HashMap;
use std::sync::Arc;

/// Resolved router. Cheap to clone; everything is reference-counted.
#[derive(Clone)]
pub struct Router {
    by_name: Arc<HashMap<String, ProviderCfg>>,
    model_routing: Arc<HashMap<String, String>>,
    default_provider: Option<String>,
    fallback_chain: Arc<Vec<String>>,
}

impl Router {
    pub fn build(cfg: &LlmGatewayConfig) -> Result<Self, String> {
        if cfg.providers.is_empty() {
            return Err("llm-gateway: at least one provider is required".into());
        }
        let mut by_name = HashMap::with_capacity(cfg.providers.len());
        for p in &cfg.providers {
            if by_name.contains_key(&p.name) {
                return Err(format!("llm-gateway: duplicate provider name `{}`", p.name));
            }
            by_name.insert(p.name.clone(), p.clone());
        }

        // Resolve default. If the operator named one explicitly, validate it
        // exists. Otherwise pick the first declared provider — same convention
        // as Kong's upstream picker.
        let default_provider = match &cfg.default_provider {
            Some(name) => {
                if !by_name.contains_key(name) {
                    return Err(format!(
                        "llm-gateway: default_provider `{name}` is not in providers list"
                    ));
                }
                Some(name.clone())
            }
            None => Some(cfg.providers[0].name.clone()),
        };

        // Validate model_routing references known providers.
        for (model, provider) in &cfg.model_routing {
            if !by_name.contains_key(provider) {
                return Err(format!(
                    "llm-gateway: model_routing[{model}] -> unknown provider `{provider}`"
                ));
            }
        }

        // De-duplicate the fallback chain while preserving order; drop unknown
        // entries with a warning rather than failing the whole plugin build.
        let mut fallback_chain = Vec::with_capacity(cfg.fallback_chain.len());
        for name in &cfg.fallback_chain {
            if !by_name.contains_key(name) {
                tracing::warn!(
                    provider = %name,
                    "llm-gateway: fallback_chain references unknown provider; ignoring"
                );
                continue;
            }
            if !fallback_chain.contains(name) {
                fallback_chain.push(name.clone());
            }
        }

        Ok(Self {
            by_name: Arc::new(by_name),
            model_routing: Arc::new(cfg.model_routing.clone()),
            default_provider,
            fallback_chain: Arc::new(fallback_chain),
        })
    }

    /// Pick the primary provider for a given model.
    ///
    /// Order of resolution:
    /// 1. exact-match in `model_routing`,
    /// 2. `default_provider`,
    /// 3. first declared provider (always non-empty after `build`).
    pub fn pick(&self, model: &str) -> &ProviderCfg {
        if let Some(name) = self.model_routing.get(model) {
            if let Some(p) = self.by_name.get(name) {
                return p;
            }
        }
        if let Some(name) = &self.default_provider {
            if let Some(p) = self.by_name.get(name) {
                return p;
            }
        }
        // Build invariant guarantees this is non-empty.
        self.by_name.values().next().expect("router has providers")
    }

    /// Build the ordered list of providers to try for a request, given the
    /// primary pick. The primary always comes first; the fallback chain is
    /// appended with any duplicates removed.
    pub fn dispatch_order(&self, primary: &str) -> Vec<ProviderCfg> {
        let mut out: Vec<ProviderCfg> = Vec::with_capacity(self.fallback_chain.len() + 1);
        if let Some(p) = self.by_name.get(primary) {
            out.push(p.clone());
        }
        for name in self.fallback_chain.iter() {
            if name == primary {
                continue;
            }
            if let Some(p) = self.by_name.get(name) {
                out.push(p.clone());
            }
        }
        out
    }
}
