//! Static config shape for the request-recorder plugin.

use serde::{Deserialize, Serialize};

fn default_dir() -> String {
    if let Some(home) = dirs::home_dir() {
        home.join(".lynxx")
            .join("recordings")
            .to_string_lossy()
            .into_owned()
    } else {
        "/var/lib/lynxx/recordings".into()
    }
}
fn default_sample_rate() -> f64 {
    0.05
}
fn default_max_body_bytes() -> usize {
    1024 * 1024
}
fn default_redact_headers() -> Vec<String> {
    vec!["Authorization".into(), "Cookie".into(), "X-API-Key".into()]
}
fn default_redact_body_paths() -> Vec<String> {
    vec!["password".into(), "credit_card".into(), "ssn".into()]
}
fn default_min_status() -> u16 {
    0
}
fn default_max_status() -> u16 {
    599
}
fn default_retention_days() -> u32 {
    7
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default = "default_dir")]
    pub directory: String,
    #[serde(default = "default_sample_rate")]
    pub sample_rate: f64,
    #[serde(default = "default_max_body_bytes")]
    pub max_body_bytes: usize,
    #[serde(default = "default_redact_headers")]
    pub redact_headers: Vec<String>,
    #[serde(default = "default_redact_body_paths")]
    pub redact_body_paths: Vec<String>,
    #[serde(default = "default_min_status")]
    pub min_status_to_record: u16,
    #[serde(default = "default_max_status")]
    pub max_status_to_record: u16,
    #[serde(default = "default_retention_days")]
    pub retention_days: u32,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            directory: default_dir(),
            sample_rate: default_sample_rate(),
            max_body_bytes: default_max_body_bytes(),
            redact_headers: default_redact_headers(),
            redact_body_paths: default_redact_body_paths(),
            min_status_to_record: default_min_status(),
            max_status_to_record: default_max_status(),
            retention_days: default_retention_days(),
        }
    }
}
