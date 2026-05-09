use crate::error::{GatewayError, PluginError};
use crate::plugins::{BoxBody, Plugin};
use async_trait::async_trait;
use axum::body::Body;
use axum::http::{Request, Response};

pub struct AclPlugin {
    pub allowed_consumers: Vec<String>,
}

impl AclPlugin {
    pub fn from_config(cfg: &serde_json::Value) -> Result<Self, GatewayError> {
        let allowed_consumers = cfg
            .get("allowed_consumers")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();
        Ok(Self { allowed_consumers })
    }
}

#[async_trait]
impl Plugin for AclPlugin {
    fn name(&self) -> &str {
        "acl"
    }

    async fn on_request(&self, req: &mut Request<Body>) -> Result<(), PluginError> {
        let consumer = req
            .headers()
            .get("x-consumer-username")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("anonymous");

        if self.allowed_consumers.is_empty() {
            return Ok(());
        }

        if !self.allowed_consumers.contains(&consumer.to_string()) {
            return Err(PluginError::ShortCircuit {
                status: 403,
                headers: vec![("content-type".into(), "application/json".into())],
                body: br#"{"error":"forbidden by acl"}"#.to_vec().into(),
            });
        }

        Ok(())
    }

    async fn on_response(&self, _resp: &mut Response<BoxBody>) -> Result<(), PluginError> {
        Ok(())
    }
}
