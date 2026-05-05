use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpenApiDriftConfig {
    /// Filesystem path to the spec. One of `spec_path` / `spec_url` must
    /// be set; if both are set, `spec_path` wins (zero-network start-up).
    #[serde(default)]
    pub spec_path: Option<String>,
    /// HTTP(S) URL to fetch the spec from at construction time. We do not
    /// hot-reload — operators redeploy the plugin to pick up a new spec.
    #[serde(default)]
    pub spec_url: Option<String>,
    /// Per-request probability that we run the validator. Set to 1.0 in
    /// dev to catch everything, 0.01-0.1 in prod to keep CPU bounded.
    #[serde(default = "default_sample_rate")]
    pub sample_rate: f64,
    /// Token-bucket cap on violation logs/metrics, per route. 0 = unlimited.
    #[serde(default = "default_max_per_min")]
    pub max_violations_per_minute: u32,
    /// `debug | info | warn | error`. Controls only the log line; the
    /// Prometheus counter is always emitted.
    #[serde(default = "default_log_level")]
    pub log_level: String,
    /// Cosmetic: when true (the default) violations are logged at the
    /// configured level; when false, drop the log call but keep the metric.
    /// Useful when you only want to graph drift without spamming logs.
    #[serde(default = "default_alert")]
    pub alert_on_violation: bool,
}

fn default_sample_rate() -> f64 {
    0.1
}

fn default_max_per_min() -> u32 {
    100
}

fn default_log_level() -> String {
    "warn".into()
}

fn default_alert() -> bool {
    true
}
