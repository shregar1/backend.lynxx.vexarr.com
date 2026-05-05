//! Load an OpenAPI spec from disk or HTTP at plugin-build time.
//!
//! We deliberately block here. Plugin construction runs once per
//! plugin-install (and on rebuild) — we'd rather pay a synchronous
//! `reqwest::blocking` cost than thread an async closure through the
//! `from_config` signature, which is shared by every plugin in the
//! gateway.

use super::config::OpenApiDriftConfig;
use crate::error::GatewayError;

pub fn load(cfg: &OpenApiDriftConfig) -> Result<String, GatewayError> {
    if let Some(path) = &cfg.spec_path {
        return std::fs::read_to_string(path)
            .map_err(|e| GatewayError::BadRequest(format!("openapi-drift: read {path}: {e}")));
    }
    if let Some(url) = &cfg.spec_url {
        return fetch_url(url);
    }
    Err(GatewayError::BadRequest(
        "openapi-drift: one of `spec_path` or `spec_url` must be set".into(),
    ))
}

fn fetch_url(url: &str) -> Result<String, GatewayError> {
    // We're called from `from_config`, which runs on a Tokio worker thread.
    // We don't enable the `blocking` feature on `reqwest`, so we drive an
    // async fetch on the current runtime and block on it. A 10-second
    // timeout caps any hung-server stall at plugin-install time.
    let url_owned = url.to_string();
    let result = tokio::task::block_in_place(|| {
        tokio::runtime::Handle::current().block_on(async move {
            let client = reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(10))
                .build()
                .map_err(|e| format!("http client: {e}"))?;
            let resp = client
                .get(&url_owned)
                .send()
                .await
                .map_err(|e| format!("GET {url_owned}: {e}"))?;
            let status = resp.status();
            let text = resp
                .text()
                .await
                .map_err(|e| format!("body of {url_owned}: {e}"))?;
            if !status.is_success() {
                return Err(format!("{url_owned} returned HTTP {status}"));
            }
            Ok::<String, String>(text)
        })
    });
    result.map_err(|e| GatewayError::BadRequest(format!("openapi-drift: {e}")))
}
