//! Response header filter plugin — selectively include or exclude
//! upstream response headers before sending to the client.
//!
//! ## Config
//!
//! ```json
//! {
//!   "allow": ["Content-Type", "Cache-Control", "ETag"],
//!   "deny": ["X-Powered-By", "Server", "X-AspNet-Version"],
//!   "mode": "deny"
//! }
//! ```
//!
//! `mode`:
//!   - `"deny"` (default): remove headers listed in `deny`, keep everything else.
//!   - `"allow"`: only keep headers listed in `allow`, remove everything else.

use crate::error::{GatewayError, PluginError};
use crate::plugins::{BoxBody, Plugin};
use axum::body::Body;
use axum::http::{Request, Response};

pub struct ResponseFilterPlugin {
    allow: Vec<String>,
    deny: Vec<String>,
    mode: FilterMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FilterMode {
    Allow,
    Deny,
}

impl ResponseFilterPlugin {
    pub fn from_config(config: &serde_json::Value) -> Result<Self, GatewayError> {
        let allow: Vec<String> = config["allow"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_lowercase()))
                    .collect()
            })
            .unwrap_or_default();

        let deny: Vec<String> = config["deny"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_lowercase()))
                    .collect()
            })
            .unwrap_or_default();

        let mode = match config["mode"].as_str().unwrap_or("deny") {
            "allow" => FilterMode::Allow,
            _ => FilterMode::Deny,
        };

        Ok(Self { allow, deny, mode })
    }
}

#[async_trait::async_trait]
impl Plugin for ResponseFilterPlugin {
    fn name(&self) -> &'static str {
        "response-filter"
    }

    async fn on_request(&self, _req: &mut Request<Body>) -> Result<(), PluginError> {
        Ok(())
    }

    async fn on_response(&self, resp: &mut Response<BoxBody>) -> Result<(), PluginError> {
        match self.mode {
            FilterMode::Deny => {
                for header_name in &self.deny {
                    resp.headers_mut().remove(header_name.as_str());
                }
            }
            FilterMode::Allow => {
                let headers = resp.headers().clone();
                let to_remove: Vec<_> = headers
                    .keys()
                    .filter(|k| !self.allow.contains(&k.as_str().to_lowercase()))
                    .cloned()
                    .collect();
                for name in to_remove {
                    resp.headers_mut().remove(&name);
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deny_mode() {
        let cfg = serde_json::json!({
            "deny": ["x-powered-by", "server"],
            "mode": "deny"
        });
        let plugin = ResponseFilterPlugin::from_config(&cfg).unwrap();
        assert_eq!(plugin.mode, FilterMode::Deny);
        assert_eq!(plugin.deny.len(), 2);
    }

    #[test]
    fn allow_mode() {
        let cfg = serde_json::json!({
            "allow": ["content-type"],
            "mode": "allow"
        });
        let plugin = ResponseFilterPlugin::from_config(&cfg).unwrap();
        assert_eq!(plugin.mode, FilterMode::Allow);
        assert_eq!(plugin.allow.len(), 1);
    }
}
