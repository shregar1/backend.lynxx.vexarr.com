use kube::CustomResource;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

fn default_retries() -> u32 {
    3
}
fn default_timeout() -> u64 {
    5000
}
fn default_weight() -> u32 {
    1
}
fn default_true() -> bool {
    true
}

#[derive(Serialize, Deserialize, Clone, Debug, JsonSchema)]
pub struct LynxxTarget {
    pub url: String,
    #[serde(default = "default_weight")]
    pub weight: u32,
}

#[derive(CustomResource, Serialize, Deserialize, Clone, Debug, JsonSchema)]
#[kube(
    group = "lynxx.dev",
    version = "v1",
    kind = "LynxxService",
    namespaced,
    shortname = "lsvc"
)]
#[serde(rename_all = "camelCase")]
pub struct LynxxServiceSpec {
    pub url: String,
    #[serde(default = "default_retries")]
    pub retries: u32,
    #[serde(default = "default_timeout")]
    pub connect_timeout_ms: u64,
    #[serde(default)]
    pub lb_strategy: Option<String>,
    #[serde(default)]
    pub targets: Vec<LynxxTarget>,
}

#[derive(CustomResource, Serialize, Deserialize, Clone, Debug, JsonSchema)]
#[kube(
    group = "lynxx.dev",
    version = "v1",
    kind = "LynxxRoute",
    namespaced,
    shortname = "lrt"
)]
#[serde(rename_all = "camelCase")]
pub struct LynxxRouteSpec {
    /// Name of the LynxxService in the same namespace.
    pub service_ref: String,
    pub paths: Vec<String>,
    #[serde(default)]
    pub methods: Vec<String>,
    #[serde(default)]
    pub strip_path: bool,
}

#[derive(CustomResource, Serialize, Deserialize, Clone, Debug, JsonSchema)]
#[kube(
    group = "lynxx.dev",
    version = "v1",
    kind = "LynxxPlugin",
    namespaced,
    shortname = "lpl"
)]
#[serde(rename_all = "camelCase")]
pub struct LynxxPluginSpec {
    /// Plugin type name (e.g. "jwt", "rate-limit").
    pub name: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// "global" | "service" | "route"
    pub scope: String,
    /// Name of the service or route resource (required when scope != global).
    #[serde(default)]
    pub scope_ref: Option<String>,
    #[serde(default)]
    pub config: serde_json::Value,
}
