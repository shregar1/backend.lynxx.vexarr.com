use crate::db::Repository;
use crate::error::{GatewayError, PluginError};
use crate::plugins::{BoxBody, Plugin};
use async_trait::async_trait;
use axum::http::{Request, Response};
use std::collections::HashSet;
use std::sync::Arc;

/// Allow/deny a request based on the consumer username injected by an
/// upstream auth plugin (jwt / basic-auth / key-auth / hmac-auth /
/// oauth2-introspect / session).
///
/// **Plugin order matters.** ACL reads `X-Consumer-Username` from the
/// request. Configure plugin priorities so an auth plugin runs *before*
/// this one — otherwise the header won't be set and ACL will deny
/// everything.
pub struct AclPlugin {
    allow: HashSet<String>,
    deny: HashSet<String>,
}

impl AclPlugin {
    pub fn from_config(
        cfg: &serde_json::Value,
        _repo: Option<Arc<Repository>>,
    ) -> Result<Self, GatewayError> {
        let allow = parse_list(cfg.get("allow"));
        let deny = parse_list(cfg.get("deny"));
        if allow.is_empty() && deny.is_empty() {
            return Err(GatewayError::BadRequest(
                "acl: at least one of `allow` or `deny` must be non-empty".into(),
            ));
        }
        Ok(Self { allow, deny })
    }
}

fn parse_list(v: Option<&serde_json::Value>) -> HashSet<String> {
    v.and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

fn forbidden() -> PluginError {
    // Returned as ShortCircuit so the body matches the spec exactly
    // (`{"error":"forbidden by acl"}`) rather than going through the
    // generic 403 envelope.
    PluginError::ShortCircuit {
        status: 403,
        headers: vec![("content-type".into(), "application/json".into())],
        body: br#"{"error":"forbidden by acl"}"#.to_vec(),
    }
}

#[async_trait]
impl Plugin for AclPlugin {
    fn name(&self) -> &str {
        "acl"
    }

    async fn on_request(
        &self,
        req: &mut Request<axum::body::Body>,
    ) -> Result<(), PluginError> {
        let username = req
            .headers()
            .get("x-consumer-username")
            .and_then(|v| v.to_str().ok())
            .ok_or_else(forbidden)?
            .to_string();

        // deny wins over allow
        if !self.deny.is_empty() && self.deny.contains(&username) {
            return Err(forbidden());
        }
        if !self.allow.is_empty() && !self.allow.contains(&username) {
            return Err(forbidden());
        }
        Ok(())
    }

    async fn on_response(&self, _resp: &mut Response<BoxBody>) -> Result<(), PluginError> {
        Ok(())
    }
}
