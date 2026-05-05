use crate::error::{GatewayError, PluginError};
use crate::plugins::{BoxBody, Plugin};
use async_trait::async_trait;
use axum::http::header::{HeaderName, HeaderValue};
use axum::http::{Request, Response, Uri};
use std::str::FromStr;

/// Send a percentage of traffic to an alternate upstream target. Routing is
/// either explicit (header match) or randomized. Malformed configuration
/// fails fast at build time; runtime errors silently fall through to the
/// primary target so a busted canary never breaks production traffic.
pub struct CanaryPlugin {
    header_name: Option<HeaderName>,
    header_value_match: Option<String>,
    alt_authority: Option<axum::http::uri::Authority>,
    alt_scheme: Option<axum::http::uri::Scheme>,
    threshold: u8,
}

impl CanaryPlugin {
    pub fn from_config(cfg: &serde_json::Value) -> Result<Self, GatewayError> {
        let header_name = cfg
            .get("header_name")
            .and_then(|v| v.as_str())
            .and_then(|s| HeaderName::from_str(s).ok());
        let header_value_match = cfg
            .get("header_value_match")
            .and_then(|v| v.as_str())
            .map(String::from);
        let alt = cfg
            .get("alternate_target_url")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                GatewayError::BadRequest("canary: `alternate_target_url` is required".into())
            })?;
        let percentage = cfg.get("percentage").and_then(|v| v.as_u64()).unwrap_or(0);
        let percentage = percentage.min(100) as u8;
        // 100% -> 255 threshold; rand byte < threshold means picked. Use saturating math.
        let threshold = ((percentage as u16 * 255) / 100) as u8;

        let parsed = Uri::from_str(alt).map_err(|e| {
            GatewayError::BadRequest(format!("canary: invalid alternate_target_url: {e}"))
        })?;
        let alt_authority = parsed.authority().cloned();
        let alt_scheme = parsed.scheme().cloned();

        Ok(Self {
            header_name,
            header_value_match,
            alt_authority,
            alt_scheme,
            threshold,
        })
    }

    fn should_route_to_canary(&self, req: &Request<axum::body::Body>) -> bool {
        // Header match wins.
        if let (Some(hn), Some(want)) = (&self.header_name, &self.header_value_match) {
            if let Some(v) = req.headers().get(hn).and_then(|v| v.to_str().ok()) {
                if v == want.as_str() {
                    return true;
                }
                // Header present but mismatched -> route to primary.
                return false;
            }
        }
        // Header absent (or no match config) -> roll the dice.
        if self.threshold == 0 {
            return false;
        }
        rand::random::<u8>() < self.threshold
    }
}

#[async_trait]
impl Plugin for CanaryPlugin {
    fn name(&self) -> &str {
        "canary"
    }

    async fn on_request(&self, req: &mut Request<axum::body::Body>) -> Result<(), PluginError> {
        if !self.should_route_to_canary(req) {
            return Ok(());
        }
        let Some(authority) = self.alt_authority.clone() else {
            tracing::debug!("canary: no alt authority parsed; skipping");
            return Ok(());
        };
        let mut parts = req.uri().clone().into_parts();
        parts.authority = Some(authority.clone());
        if let Some(scheme) = self.alt_scheme.clone() {
            parts.scheme = Some(scheme);
        }
        // Path-and-query is preserved.
        match Uri::from_parts(parts) {
            Ok(new_uri) => {
                tracing::debug!(target = %new_uri, "canary: routing to alternate target");
                *req.uri_mut() = new_uri;
                // Mark the request so downstream plugins / proxy can see the decision.
                if let Ok(v) = HeaderValue::from_str(authority.as_str()) {
                    req.headers_mut().insert("x-lynxx-canary", v);
                }
            }
            Err(e) => {
                tracing::debug!(error = %e, "canary: failed to rebuild URI; falling through");
            }
        }
        Ok(())
    }

    async fn on_response(&self, _resp: &mut Response<BoxBody>) -> Result<(), PluginError> {
        Ok(())
    }
}
