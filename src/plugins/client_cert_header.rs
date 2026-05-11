//! Client certificate header injection plugin — propagate mTLS client
//! certificate details to upstream via request headers.
//!
//! ## Config
//!
//! ```json
//! {
//!   "cn_header": "X-Client-Cert-CN",
//!   "fingerprint_header": "X-Client-Cert-Fingerprint",
//!   "serial_header": "X-Client-Cert-Serial",
//!   "dn_header": "X-Client-Cert-DN",
//!   "cert_header": "X-Client-Cert"
//! }
//! ```
//!
//! All fields optional. When set, the corresponding header is populated
//! from the peer certificate presented during the TLS handshake.

use crate::error::{GatewayError, PluginError};
use crate::plugins::{BoxBody, Plugin};
use axum::body::Body;
use axum::http::{Request, Response};

/// Request extension set by the TLS acceptor when a client cert is presented.
#[derive(Clone, Debug)]
pub struct ClientCertInfo {
    pub common_name: String,
    pub fingerprint_sha256: String,
    pub serial: String,
    pub distinguished_name: String,
    pub pem: String,
}

pub struct ClientCertHeaderPlugin {
    cn_header: Option<String>,
    fingerprint_header: Option<String>,
    serial_header: Option<String>,
    dn_header: Option<String>,
    cert_header: Option<String>,
}

impl ClientCertHeaderPlugin {
    pub fn from_config(config: &serde_json::Value) -> Result<Self, GatewayError> {
        Ok(Self {
            cn_header: config["cn_header"].as_str().map(|s| s.to_string()),
            fingerprint_header: config["fingerprint_header"].as_str().map(|s| s.to_string()),
            serial_header: config["serial_header"].as_str().map(|s| s.to_string()),
            dn_header: config["dn_header"].as_str().map(|s| s.to_string()),
            cert_header: config["cert_header"].as_str().map(|s| s.to_string()),
        })
    }

    fn set_header(req: &mut Request<Body>, name: &str, value: &str) {
        if let (Ok(n), Ok(v)) = (
            axum::http::header::HeaderName::from_bytes(name.as_bytes()),
            axum::http::header::HeaderValue::from_str(value),
        ) {
            req.headers_mut().insert(n, v);
        }
    }
}

#[async_trait::async_trait]
impl Plugin for ClientCertHeaderPlugin {
    fn name(&self) -> &'static str {
        "client-cert-header"
    }

    async fn on_request(&self, req: &mut Request<Body>) -> Result<(), PluginError> {
        let cert_info = match req.extensions().get::<ClientCertInfo>() {
            Some(info) => info.clone(),
            None => return Ok(()), // No client cert — nothing to inject.
        };

        if let Some(ref h) = self.cn_header {
            Self::set_header(req, h, &cert_info.common_name);
        }
        if let Some(ref h) = self.fingerprint_header {
            Self::set_header(req, h, &cert_info.fingerprint_sha256);
        }
        if let Some(ref h) = self.serial_header {
            Self::set_header(req, h, &cert_info.serial);
        }
        if let Some(ref h) = self.dn_header {
            Self::set_header(req, h, &cert_info.distinguished_name);
        }
        if let Some(ref h) = self.cert_header {
            // URL-encode the PEM so it fits in a single header value.
            let encoded = cert_info.pem.replace('\n', "%0A");
            Self::set_header(req, h, &encoded);
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
    fn parse_config() {
        let cfg = serde_json::json!({
            "cn_header": "X-Client-CN",
            "fingerprint_header": "X-Client-FP"
        });
        let plugin = ClientCertHeaderPlugin::from_config(&cfg).unwrap();
        assert_eq!(plugin.cn_header.as_deref(), Some("X-Client-CN"));
        assert_eq!(plugin.fingerprint_header.as_deref(), Some("X-Client-FP"));
        assert!(plugin.serial_header.is_none());
    }
}
