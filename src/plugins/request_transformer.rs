use crate::error::{GatewayError, PluginError};
use crate::plugins::{BoxBody, Plugin};
use async_trait::async_trait;
use axum::http::header::{HeaderMap, HeaderName, HeaderValue};
use axum::http::{Request, Response};
use std::str::FromStr;

pub struct RequestTransformerPlugin {
    add: Vec<(HeaderName, HeaderValue)>,
    remove: Vec<HeaderName>,
    replace: Vec<(HeaderName, HeaderValue)>,
}

impl RequestTransformerPlugin {
    pub fn from_config(cfg: &serde_json::Value) -> Result<Self, GatewayError> {
        Ok(Self {
            add: parse_header_map(cfg.get("add_headers"))?,
            remove: parse_header_list(cfg.get("remove_headers"))?,
            replace: parse_header_map(cfg.get("replace_headers"))?,
        })
    }
}

pub(crate) fn parse_header_map(
    val: Option<&serde_json::Value>,
) -> Result<Vec<(HeaderName, HeaderValue)>, GatewayError> {
    let Some(obj) = val.and_then(|v| v.as_object()) else {
        return Ok(Vec::new());
    };
    let mut out = Vec::with_capacity(obj.len());
    for (k, v) in obj {
        let name = HeaderName::from_str(k)
            .map_err(|e| GatewayError::BadRequest(format!("invalid header name {k}: {e}")))?;
        let s = v
            .as_str()
            .ok_or_else(|| GatewayError::BadRequest(format!("header {k} value must be string")))?;
        let value = HeaderValue::from_str(s)
            .map_err(|e| GatewayError::BadRequest(format!("invalid header value for {k}: {e}")))?;
        out.push((name, value));
    }
    Ok(out)
}

pub(crate) fn parse_header_list(
    val: Option<&serde_json::Value>,
) -> Result<Vec<HeaderName>, GatewayError> {
    let Some(arr) = val.and_then(|v| v.as_array()) else {
        return Ok(Vec::new());
    };
    let mut out = Vec::with_capacity(arr.len());
    for v in arr {
        let s = v
            .as_str()
            .ok_or_else(|| GatewayError::BadRequest("header name must be string".into()))?;
        let name = HeaderName::from_str(s)
            .map_err(|e| GatewayError::BadRequest(format!("invalid header name {s}: {e}")))?;
        out.push(name);
    }
    Ok(out)
}

pub(crate) fn apply_transforms(
    headers: &mut HeaderMap,
    add: &[(HeaderName, HeaderValue)],
    remove: &[HeaderName],
    replace: &[(HeaderName, HeaderValue)],
) {
    for name in remove {
        headers.remove(name);
    }
    for (name, value) in add {
        headers.append(name.clone(), value.clone());
    }
    for (name, value) in replace {
        headers.insert(name.clone(), value.clone());
    }
}

#[async_trait]
impl Plugin for RequestTransformerPlugin {
    fn name(&self) -> &str {
        "request-transformer"
    }

    async fn on_request(
        &self,
        req: &mut Request<axum::body::Body>,
    ) -> Result<(), PluginError> {
        apply_transforms(req.headers_mut(), &self.add, &self.remove, &self.replace);
        Ok(())
    }

    async fn on_response(&self, _resp: &mut Response<BoxBody>) -> Result<(), PluginError> {
        Ok(())
    }
}
