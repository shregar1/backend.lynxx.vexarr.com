//! Typed config for the `graphql-guard` plugin. Every field is optional with
//! a sensible default so a bare `{}` config is valid: it does nothing
//! interesting but never breaks a request.

use serde::Deserialize;
use std::collections::HashMap;

#[derive(Debug, Clone, Deserialize)]
pub struct GraphqlGuardConfig {
    #[serde(default = "default_max_depth")]
    pub max_depth: usize,

    #[serde(default = "default_max_complexity")]
    pub max_complexity: u64,

    #[serde(default = "default_max_aliases")]
    pub max_aliases: usize,

    #[serde(default)]
    pub field_costs: HashMap<String, u64>,

    #[serde(default = "default_field_cost")]
    pub default_field_cost: u64,

    #[serde(default = "default_true")]
    pub introspection_disabled_in_production: bool,

    #[serde(default)]
    pub production_mode: bool,

    #[serde(default)]
    pub field_rate_limits: Vec<FieldRateLimit>,

    #[serde(default)]
    pub persisted_queries_only: bool,

    #[serde(default)]
    pub persisted_queries: HashMap<String, String>,

    #[serde(default)]
    pub operation_name_required: bool,

    #[serde(default)]
    pub block_mutations_for_anonymous: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FieldRateLimit {
    pub field: String,
    pub requests_per_second: u32,
    #[serde(default = "default_key_strategy")]
    pub key_strategy: String,
}

fn default_max_depth() -> usize {
    10
}
fn default_max_complexity() -> u64 {
    1000
}
fn default_max_aliases() -> usize {
    30
}
fn default_field_cost() -> u64 {
    1
}
fn default_true() -> bool {
    true
}
fn default_key_strategy() -> String {
    "consumer".into()
}

impl Default for GraphqlGuardConfig {
    fn default() -> Self {
        Self {
            max_depth: default_max_depth(),
            max_complexity: default_max_complexity(),
            max_aliases: default_max_aliases(),
            field_costs: HashMap::new(),
            default_field_cost: default_field_cost(),
            introspection_disabled_in_production: true,
            production_mode: false,
            field_rate_limits: Vec::new(),
            persisted_queries_only: false,
            persisted_queries: HashMap::new(),
            operation_name_required: false,
            block_mutations_for_anonymous: false,
        }
    }
}
