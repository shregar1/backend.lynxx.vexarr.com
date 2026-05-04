use crate::error::PluginError;
use crate::plugins::{BoxBody, Plugin};
use async_trait::async_trait;
use axum::http::{Request, Response};
use dashmap::DashMap;
use jsonwebtoken::{decode, DecodingKey, Validation};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
struct Claims {
    sub: String,
    exp: usize,
    iss: Option<String>,
}

pub struct AuthPlugin {
    api_keys: DashMap<String, String>,
    jwt_secret: String,
}

impl AuthPlugin {
    pub fn new(jwt_secret: String) -> Self {
        Self {
            api_keys: DashMap::new(),
            jwt_secret,
        }
    }

    pub fn add_api_key(&self, key: String, consumer: String) {
        self.api_keys.insert(key, consumer);
    }

    fn validate_api_key(&self, key: &str) -> Result<(), PluginError> {
        if self.api_keys.contains_key(key) {
            Ok(())
        } else {
            Err(PluginError::Unauthorized("invalid API key".into()))
        }
    }

    fn validate_jwt(&self, token: &str) -> Result<(), PluginError> {
        let key = DecodingKey::from_secret(self.jwt_secret.as_bytes());
        let validation = Validation::default();

        decode::<Claims>(token, &key, &validation)
            .map(|_| ())
            .map_err(|e| PluginError::Unauthorized(format!("invalid JWT: {e}")))
    }
}

#[async_trait]
impl Plugin for AuthPlugin {
    fn name(&self) -> &str {
        "auth"
    }

    async fn on_request(
        &self,
        req: &mut Request<axum::body::Body>,
    ) -> Result<(), PluginError> {
        if let Some(key) = req.headers().get("x-api-key") {
            let key = key
                .to_str()
                .map_err(|_| PluginError::Unauthorized("invalid header value".into()))?;
            return self.validate_api_key(key);
        }

        if let Some(auth) = req.headers().get("authorization") {
            let auth = auth
                .to_str()
                .map_err(|_| PluginError::Unauthorized("invalid header value".into()))?;

            if let Some(token) = auth.strip_prefix("Bearer ") {
                return self.validate_jwt(token);
            }
        }

        Err(PluginError::Unauthorized(
            "no credentials provided".into(),
        ))
    }

    async fn on_response(
        &self,
        _resp: &mut Response<BoxBody>,
    ) -> Result<(), PluginError> {
        Ok(())
    }
}
