use crate::error::{GatewayError, PluginError};
use crate::plugins::{BoxBody, Plugin};
use async_trait::async_trait;
use axum::http::{header, HeaderValue, Request, Response};

pub struct CorsPlugin {
    allow_origins: Vec<String>,
    allow_methods: String,
    allow_headers: String,
}

impl CorsPlugin {
    pub fn new(allow_origins: Vec<String>) -> Self {
        Self {
            allow_origins,
            allow_methods: "GET, POST, PUT, DELETE, PATCH, OPTIONS".into(),
            allow_headers: "Content-Type, Authorization, X-Api-Key, X-Request-Id".into(),
        }
    }

    pub fn from_config(cfg: &serde_json::Value) -> Result<Self, GatewayError> {
        let origins = cfg
            .get("allow_origins")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_else(|| vec!["*".to_string()]);
        Ok(Self::new(origins))
    }
}

#[async_trait]
impl Plugin for CorsPlugin {
    fn name(&self) -> &str {
        "cors"
    }

    async fn on_request(
        &self,
        _req: &mut Request<axum::body::Body>,
    ) -> Result<(), PluginError> {
        Ok(())
    }

    async fn on_response(
        &self,
        resp: &mut Response<BoxBody>,
    ) -> Result<(), PluginError> {
        let origin = if self.allow_origins.contains(&"*".to_string()) {
            "*"
        } else {
            self.allow_origins.first().map(|s| s.as_str()).unwrap_or("*")
        };

        let headers = resp.headers_mut();
        headers.insert(
            header::ACCESS_CONTROL_ALLOW_ORIGIN,
            HeaderValue::from_str(origin).unwrap_or(HeaderValue::from_static("*")),
        );
        headers.insert(
            header::ACCESS_CONTROL_ALLOW_METHODS,
            HeaderValue::from_str(&self.allow_methods)
                .unwrap_or(HeaderValue::from_static("GET")),
        );
        headers.insert(
            header::ACCESS_CONTROL_ALLOW_HEADERS,
            HeaderValue::from_str(&self.allow_headers)
                .unwrap_or(HeaderValue::from_static("Content-Type")),
        );

        Ok(())
    }
}
