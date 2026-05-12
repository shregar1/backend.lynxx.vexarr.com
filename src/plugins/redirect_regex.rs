//! Regex-based redirect plugin.
//!
//! Matches the request path against a regex and redirects to a replacement
//! URL built from capture groups. Supports 301 (permanent) and 302
//! (temporary) redirects by default, with an optional explicit status code.
//!
//! Config:
//! ```json
//! {
//!   "regex": "/api/(.*)",
//!   "replacement": "/v2/$1",
//!   "permanent": false,
//!   "status_code": 302
//! }
//! ```

use crate::error::{GatewayError, PluginError};
use crate::plugins::{BoxBody, Plugin};
use async_trait::async_trait;
use axum::body::Body;
use axum::http::header::LOCATION;
use axum::http::{HeaderValue, Request, Response, StatusCode};
use regex::Regex;

pub struct RedirectRegexPlugin {
    regex: Regex,
    replacement: String,
    status_code: StatusCode,
}

impl RedirectRegexPlugin {
    pub fn from_config(cfg: &serde_json::Value) -> Result<Self, GatewayError> {
        let pattern = cfg
            .get("regex")
            .and_then(|v| v.as_str())
            .ok_or_else(|| GatewayError::BadRequest("redirect-regex: missing 'regex'".into()))?;

        let regex = Regex::new(pattern)
            .map_err(|e| GatewayError::BadRequest(format!("redirect-regex: invalid regex: {e}")))?;

        let replacement = cfg
            .get("replacement")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                GatewayError::BadRequest("redirect-regex: missing 'replacement'".into())
            })?
            .to_string();

        let permanent = cfg
            .get("permanent")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let code = cfg
            .get("status_code")
            .and_then(|v| v.as_u64())
            .map(|v| v as u16)
            .unwrap_or(if permanent { 301 } else { 302 });

        let status_code = StatusCode::from_u16(code).map_err(|_| {
            GatewayError::BadRequest(format!("redirect-regex: invalid status code {code}"))
        })?;

        // Only redirect status codes (3xx) make sense here.
        if !status_code.is_redirection() {
            return Err(GatewayError::BadRequest(format!(
                "redirect-regex: status code {code} is not a redirect (3xx)"
            )));
        }

        Ok(Self {
            regex,
            replacement,
            status_code,
        })
    }
}

#[async_trait]
impl Plugin for RedirectRegexPlugin {
    fn name(&self) -> &str {
        "redirect-regex"
    }

    async fn on_request(&self, req: &mut Request<Body>) -> Result<(), PluginError> {
        let path = req.uri().path().to_string();

        if self.regex.is_match(&path) {
            let location = self
                .regex
                .replace(&path, self.replacement.as_str())
                .into_owned();

            req.extensions_mut().insert(RedirectRegexInfo {
                status: self.status_code,
                location,
            });
        }

        Ok(())
    }

    async fn on_response(&self, resp: &mut Response<BoxBody>) -> Result<(), PluginError> {
        if let Some(info) = resp.extensions().get::<RedirectRegexInfo>().cloned() {
            *resp.status_mut() = info.status;
            if let Ok(val) = HeaderValue::from_str(&info.location) {
                resp.headers_mut().insert(LOCATION, val);
            }
            *resp.body_mut() = Body::empty();
        }
        Ok(())
    }
}

#[derive(Clone)]
struct RedirectRegexInfo {
    status: StatusCode,
    location: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_plugin(regex: &str, replacement: &str, permanent: bool) -> RedirectRegexPlugin {
        let cfg = serde_json::json!({
            "regex": regex,
            "replacement": replacement,
            "permanent": permanent,
        });
        RedirectRegexPlugin::from_config(&cfg).unwrap()
    }

    fn make_plugin_with_status(
        regex: &str,
        replacement: &str,
        status_code: u16,
    ) -> RedirectRegexPlugin {
        let cfg = serde_json::json!({
            "regex": regex,
            "replacement": replacement,
            "status_code": status_code,
        });
        RedirectRegexPlugin::from_config(&cfg).unwrap()
    }

    // ── on_request tests ──

    #[tokio::test]
    async fn regex_capture_group_redirect() {
        let plugin = make_plugin(r"/api/(.*)", "/v2/$1", false);
        let mut req = Request::builder()
            .uri("/api/users/123")
            .body(Body::empty())
            .unwrap();

        plugin.on_request(&mut req).await.unwrap();

        let info = req.extensions().get::<RedirectRegexInfo>().unwrap();
        assert_eq!(info.location, "/v2/users/123");
        assert_eq!(info.status, StatusCode::FOUND); // 302
    }

    #[tokio::test]
    async fn non_matching_path_passes_through() {
        let plugin = make_plugin(r"^/api/(.*)", "/v2/$1", false);
        let mut req = Request::builder()
            .uri("/web/page")
            .body(Body::empty())
            .unwrap();

        plugin.on_request(&mut req).await.unwrap();

        assert!(req.extensions().get::<RedirectRegexInfo>().is_none());
    }

    #[tokio::test]
    async fn permanent_redirect_uses_301() {
        let plugin = make_plugin(r"^/old/(.*)", "/new/$1", true);
        let mut req = Request::builder()
            .uri("/old/page")
            .body(Body::empty())
            .unwrap();

        plugin.on_request(&mut req).await.unwrap();

        let info = req.extensions().get::<RedirectRegexInfo>().unwrap();
        assert_eq!(info.status, StatusCode::MOVED_PERMANENTLY); // 301
        assert_eq!(info.location, "/new/page");
    }

    #[tokio::test]
    async fn temporary_redirect_uses_302() {
        let plugin = make_plugin(r"^/temp/(.*)", "/dest/$1", false);
        let mut req = Request::builder()
            .uri("/temp/resource")
            .body(Body::empty())
            .unwrap();

        plugin.on_request(&mut req).await.unwrap();

        let info = req.extensions().get::<RedirectRegexInfo>().unwrap();
        assert_eq!(info.status, StatusCode::FOUND); // 302
    }

    #[tokio::test]
    async fn explicit_status_code_overrides_permanent() {
        let plugin = make_plugin_with_status(r"^/moved/(.*)", "/target/$1", 307);
        let mut req = Request::builder()
            .uri("/moved/data")
            .body(Body::empty())
            .unwrap();

        plugin.on_request(&mut req).await.unwrap();

        let info = req.extensions().get::<RedirectRegexInfo>().unwrap();
        assert_eq!(info.status, StatusCode::TEMPORARY_REDIRECT); // 307
        assert_eq!(info.location, "/target/data");
    }

    #[tokio::test]
    async fn full_url_regex_replacement() {
        let plugin = make_plugin(
            r"^/legacy/(.+)/(.+)$",
            "https://new.example.com/$2/$1",
            true,
        );
        let mut req = Request::builder()
            .uri("/legacy/users/profile")
            .body(Body::empty())
            .unwrap();

        plugin.on_request(&mut req).await.unwrap();

        let info = req.extensions().get::<RedirectRegexInfo>().unwrap();
        assert_eq!(info.location, "https://new.example.com/profile/users");
        assert_eq!(info.status, StatusCode::MOVED_PERMANENTLY);
    }

    // ── on_response tests ──

    #[tokio::test]
    async fn on_response_sets_location_and_status() {
        let plugin = make_plugin(r"/api/(.*)", "/v2/$1", false);

        // Simulate the redirect info being stashed in extensions.
        let mut resp = Response::builder().status(200).body(Body::empty()).unwrap();
        resp.extensions_mut().insert(RedirectRegexInfo {
            status: StatusCode::FOUND,
            location: "/v2/users".to_string(),
        });

        plugin.on_response(&mut resp).await.unwrap();

        assert_eq!(resp.status(), StatusCode::FOUND);
        assert_eq!(
            resp.headers().get(LOCATION).unwrap().to_str().unwrap(),
            "/v2/users"
        );
    }

    #[tokio::test]
    async fn on_response_no_redirect_info_is_noop() {
        let plugin = make_plugin(r"/api/(.*)", "/v2/$1", false);

        let mut resp = Response::builder().status(200).body(Body::empty()).unwrap();

        plugin.on_response(&mut resp).await.unwrap();

        // Status unchanged, no Location header added
        assert_eq!(resp.status(), StatusCode::OK);
        assert!(resp.headers().get(LOCATION).is_none());
    }

    // ── config validation ──

    #[test]
    fn missing_regex_returns_error() {
        let cfg = serde_json::json!({ "replacement": "/v2/$1" });
        assert!(RedirectRegexPlugin::from_config(&cfg).is_err());
    }

    #[test]
    fn missing_replacement_returns_error() {
        let cfg = serde_json::json!({ "regex": "/api/(.*)" });
        assert!(RedirectRegexPlugin::from_config(&cfg).is_err());
    }

    #[test]
    fn invalid_regex_returns_error() {
        let cfg = serde_json::json!({
            "regex": "[invalid(",
            "replacement": "/v2/$1",
        });
        assert!(RedirectRegexPlugin::from_config(&cfg).is_err());
    }

    #[test]
    fn invalid_status_code_returns_error() {
        let cfg = serde_json::json!({
            "regex": "/api/(.*)",
            "replacement": "/v2/$1",
            "status_code": 999,
        });
        assert!(RedirectRegexPlugin::from_config(&cfg).is_err());
    }

    #[test]
    fn plugin_name_is_redirect_regex() {
        let plugin = make_plugin(r"/api/(.*)", "/v2/$1", false);
        assert_eq!(plugin.name(), "redirect-regex");
    }
}
