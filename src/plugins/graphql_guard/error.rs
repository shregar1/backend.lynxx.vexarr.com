//! GraphQL-shaped JSON error helpers.
//!
//! GraphQL errors live under `errors[].message` with optional
//! `errors[].extensions.code` — clients (Apollo, urql, Relay) all reach for
//! `extensions.code` to drive retry / UI behaviour, so we keep them stable
//! and uppercased.

use crate::error::PluginError;
use serde_json::{json, Value};

/// Build a GraphQL error envelope with a single error.
pub fn graphql_error_body(message: &str, code: &str, extra: Option<Value>) -> Vec<u8> {
    let mut extensions = json!({ "code": code });
    if let Some(v) = extra {
        if let (Some(ext_obj), Some(extra_obj)) = (extensions.as_object_mut(), v.as_object()) {
            for (k, val) in extra_obj {
                ext_obj.insert(k.clone(), val.clone());
            }
        }
    }
    let body = json!({
        "errors": [{
            "message": message,
            "extensions": extensions,
        }]
    });
    serde_json::to_vec(&body).unwrap_or_else(|_| {
        format!(r#"{{"errors":[{{"message":"{message}","extensions":{{"code":"{code}"}}}}]}}"#)
            .into_bytes()
    })
}

/// Short-circuit response with `application/json` and a GraphQL-shaped body.
pub fn short_circuit(
    status: u16,
    message: &str,
    code: &str,
    extra_extensions: Option<Value>,
    extra_headers: Vec<(String, String)>,
) -> PluginError {
    let body = graphql_error_body(message, code, extra_extensions);
    let mut headers = vec![("content-type".into(), "application/json".into())];
    headers.extend(extra_headers);
    PluginError::ShortCircuit {
        status,
        headers,
        body,
    }
}
