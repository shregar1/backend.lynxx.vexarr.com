//! Method override plugin — change the HTTP method based on a header.
//!
//! ## Config
//!
//! ```json
//! {
//!   "header": "X-HTTP-Method-Override",
//!   "allowed_methods": ["PUT", "PATCH", "DELETE"]
//! }
//! ```
//!
//! When the specified header is present, the request method is changed to the
//! header value. Only methods in `allowed_methods` are accepted. Useful for
//! clients that can only send GET/POST (e.g., some HTML forms, firewalls that
//! block uncommon methods).

use crate::error::{GatewayError, PluginError};
use crate::plugins::{BoxBody, Plugin};
use axum::body::Body;
use axum::http::{Method, Request, Response};

pub struct MethodOverridePlugin {
    header: String,
    allowed_methods: Vec<String>,
}

impl MethodOverridePlugin {
    pub fn from_config(config: &serde_json::Value) -> Result<Self, GatewayError> {
        let header = config["header"]
            .as_str()
            .unwrap_or("X-HTTP-Method-Override")
            .to_string();

        let allowed_methods = config["allowed_methods"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_uppercase()))
                    .collect()
            })
            .unwrap_or_else(|| {
                vec![
                    "PUT".to_string(),
                    "PATCH".to_string(),
                    "DELETE".to_string(),
                    "OPTIONS".to_string(),
                ]
            });

        Ok(Self {
            header,
            allowed_methods,
        })
    }
}

#[async_trait::async_trait]
impl Plugin for MethodOverridePlugin {
    fn name(&self) -> &'static str {
        "method-override"
    }

    async fn on_request(&self, req: &mut Request<Body>) -> Result<(), PluginError> {
        if let Some(override_val) = req.headers().get(self.header.as_str()) {
            if let Ok(method_str) = override_val.to_str() {
                let upper = method_str.to_uppercase();
                if self.allowed_methods.contains(&upper) {
                    if let Ok(method) = Method::from_bytes(upper.as_bytes()) {
                        *req.method_mut() = method;
                        // Remove the override header so the upstream doesn't see it.
                        req.headers_mut().remove(self.header.as_str());
                    }
                }
            }
        }
        Ok(())
    }

    async fn on_response(&self, _resp: &mut Response<BoxBody>) -> Result<(), PluginError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults() {
        let cfg = serde_json::json!({});
        let plugin = MethodOverridePlugin::from_config(&cfg).unwrap();
        assert_eq!(plugin.header, "X-HTTP-Method-Override");
        assert!(plugin.allowed_methods.contains(&"PUT".to_string()));
    }
}
