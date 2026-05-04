use crate::error::{GatewayError, PluginError};
use crate::plugins::{BoxBody, Plugin};
use async_trait::async_trait;
use axum::http::header::{HeaderName, HeaderValue};
use axum::http::{Request, Response};
use jsonwebtoken::{decode, Algorithm, DecodingKey, Validation};
use std::str::FromStr;

/// Real JWT verification. Supports HS256 (with `secret`) and RS256 (with
/// `public_key`). On success, injects the verified claims as a JSON-encoded
/// `X-Jwt-Claims` header so downstream services can consume them without
/// re-verifying.
pub struct JwtPlugin {
    decoding_key: DecodingKey,
    validation: Validation,
    header_name: HeaderName,
    scheme: String,
    claims_required: Vec<String>,
}

impl JwtPlugin {
    pub fn from_config(cfg: &serde_json::Value) -> Result<Self, GatewayError> {
        let header = cfg
            .get("header")
            .and_then(|v| v.as_str())
            .unwrap_or("Authorization");
        let header_name = HeaderName::from_str(header)
            .map_err(|e| GatewayError::BadRequest(format!("invalid header name: {e}")))?;
        let scheme = cfg
            .get("scheme")
            .and_then(|v| v.as_str())
            .unwrap_or("Bearer")
            .to_string();
        let alg_str = cfg
            .get("algorithm")
            .and_then(|v| v.as_str())
            .unwrap_or("HS256");
        let algorithm = match alg_str {
            "HS256" => Algorithm::HS256,
            "HS384" => Algorithm::HS384,
            "HS512" => Algorithm::HS512,
            "RS256" => Algorithm::RS256,
            "RS384" => Algorithm::RS384,
            "RS512" => Algorithm::RS512,
            other => {
                return Err(GatewayError::BadRequest(format!(
                    "unsupported jwt algorithm: {other}"
                )))
            }
        };

        // RS* uses public_key (PEM); HS* uses secret bytes. If `public_key` is
        // present we treat it as RS256 by default unless the caller asked for
        // an explicit RS variant.
        let decoding_key = if let Some(pk) = cfg.get("public_key").and_then(|v| v.as_str()) {
            DecodingKey::from_rsa_pem(pk.as_bytes()).map_err(|e| {
                GatewayError::BadRequest(format!("invalid jwt public_key PEM: {e}"))
            })?
        } else {
            let secret = cfg.get("secret").and_then(|v| v.as_str()).unwrap_or("");
            DecodingKey::from_secret(secret.as_bytes())
        };

        let mut validation = Validation::new(algorithm);
        // We perform our own required-claims check below so we can name the
        // missing claim in the error.
        validation.required_spec_claims.clear();
        validation.validate_exp = true;

        let claims_required = cfg
            .get("claims_required")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_else(|| vec!["sub".into(), "exp".into()]);

        Ok(Self {
            decoding_key,
            validation,
            header_name,
            scheme,
            claims_required,
        })
    }

    fn extract_token<'a>(&self, raw: &'a str) -> Option<&'a str> {
        if self.scheme.is_empty() {
            return Some(raw.trim());
        }
        let prefix = format!("{} ", self.scheme);
        raw.strip_prefix(&prefix).map(str::trim)
    }
}

#[async_trait]
impl Plugin for JwtPlugin {
    fn name(&self) -> &str {
        "jwt"
    }

    async fn on_request(
        &self,
        req: &mut Request<axum::body::Body>,
    ) -> Result<(), PluginError> {
        let raw = req
            .headers()
            .get(&self.header_name)
            .and_then(|v| v.to_str().ok())
            .ok_or_else(|| PluginError::Unauthorized("missing JWT header".into()))?;
        let token = self
            .extract_token(raw)
            .ok_or_else(|| PluginError::Unauthorized("malformed JWT header".into()))?;

        let data = decode::<serde_json::Value>(token, &self.decoding_key, &self.validation)
            .map_err(|e| PluginError::Unauthorized(format!("invalid JWT: {e}")))?;

        for claim in &self.claims_required {
            if data.claims.get(claim).is_none() {
                return Err(PluginError::Unauthorized(format!(
                    "missing required claim: {claim}"
                )));
            }
        }

        let claims_json = serde_json::to_string(&data.claims)
            .map_err(|e| PluginError::Internal(format!("claims serialize: {e}")))?;
        if let Ok(value) = HeaderValue::from_str(&claims_json) {
            req.headers_mut().insert("x-jwt-claims", value);
        }
        Ok(())
    }

    async fn on_response(&self, _resp: &mut Response<BoxBody>) -> Result<(), PluginError> {
        Ok(())
    }
}
