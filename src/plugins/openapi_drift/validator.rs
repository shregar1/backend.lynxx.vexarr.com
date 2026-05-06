//! Compile-once response-schema validators keyed by `(method, path_template)`.
//!
//! We pre-compile every operation's response schemas at plugin build, so the
//! per-request hot path is: match path template (linear scan, but with a
//! short-circuit on segment count), look up `(status → JSONSchema)`, run
//! `schema.validate(&body)`. No spec re-parse, no schema recompile.

use super::config::OpenApiDriftConfig;
use crate::error::GatewayError;
use jsonschema::JSONSchema;
use openapiv3::{OpenAPI, ReferenceOr};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Instant;

/// One compiled response-shape rule: method+path matches → status code → schema.
struct CompiledOp {
    /// HTTP method (uppercased).
    method: String,
    /// Path template segments. `{name}` segments are wildcards.
    segments: Vec<Seg>,
    /// status (or 0 for `default`) → compiled schema
    schemas: HashMap<u16, JSONSchema>,
    /// status code 0 holds the `default` schema, used as a fallback.
    has_default: bool,
}

enum Seg {
    Literal(String),
    Wild,
}

/// Token bucket per (method, path_template) — one bucket per route.
struct Bucket {
    /// Tokens remaining in the current minute.
    tokens: u32,
    /// Wall-clock start of the current minute.
    epoch: Instant,
    /// Cap (refill amount).
    cap: u32,
}

impl Bucket {
    fn new(cap: u32) -> Self {
        Self {
            tokens: cap,
            epoch: Instant::now(),
            cap,
        }
    }

    /// Try to consume a token. Returns true if logging should proceed.
    fn try_consume(&mut self) -> bool {
        if self.cap == 0 {
            return true; // unlimited
        }
        let now = Instant::now();
        if now.duration_since(self.epoch).as_secs() >= 60 {
            self.tokens = self.cap;
            self.epoch = now;
        }
        if self.tokens > 0 {
            self.tokens -= 1;
            true
        } else {
            false
        }
    }
}

pub struct DriftValidator {
    ops: Vec<CompiledOp>,
    buckets: Mutex<HashMap<String, Bucket>>,
    bucket_cap: u32,
    log_level: LogLevel,
    alert_on_violation: bool,
    /// Total recorded violations — exposed for tests.
    violations_total: AtomicU64,
}

#[derive(Clone, Copy)]
enum LogLevel {
    Debug,
    Info,
    Warn,
    Error,
}

impl LogLevel {
    fn parse(s: &str) -> Self {
        match s.to_ascii_lowercase().as_str() {
            "debug" => LogLevel::Debug,
            "info" => LogLevel::Info,
            "error" => LogLevel::Error,
            _ => LogLevel::Warn,
        }
    }
}

fn parse_segments(template: &str) -> Vec<Seg> {
    template
        .split('/')
        .filter(|s| !s.is_empty())
        .map(|s| {
            if s.starts_with('{') && s.ends_with('}') {
                Seg::Wild
            } else {
                Seg::Literal(s.to_string())
            }
        })
        .collect()
}

fn schema_for(media: &openapiv3::MediaType) -> Option<serde_json::Value> {
    let schema_ref = media.schema.as_ref()?;
    match schema_ref {
        ReferenceOr::Reference { reference: _ } => {
            // We don't dereference — the openapiv3 crate doesn't expose a
            // resolver, and inlining $ref by hand is out of scope for v1.
            // Operators who care should bundle the spec via redocly-cli or
            // similar before pointing the plugin at it.
            None
        }
        ReferenceOr::Item(_) => {
            // Round-trip via serde to get raw JSON Schema. The
            // `openapiv3::Schema` is a structured type; serializing it
            // produces the JSON Schema we feed to `jsonschema`.
            serde_json::to_value(schema_ref).ok()
        }
    }
}

impl DriftValidator {
    pub fn build(raw_spec: &str, cfg: &OpenApiDriftConfig) -> Result<Self, GatewayError> {
        // Try JSON first (strictly faster), fall back to YAML.
        let spec: OpenAPI = match serde_json::from_str::<OpenAPI>(raw_spec) {
            Ok(s) => s,
            Err(_) => serde_yaml::from_str::<OpenAPI>(raw_spec)
                .map_err(|e| GatewayError::BadRequest(format!("openapi-drift: parse: {e}")))?,
        };

        let mut ops = Vec::new();
        for (template, item_ref) in &spec.paths.paths {
            let item = match item_ref {
                ReferenceOr::Item(it) => it,
                _ => continue,
            };
            let segments = parse_segments(template);
            for (method_name, op_opt) in [
                ("GET", &item.get),
                ("POST", &item.post),
                ("PUT", &item.put),
                ("DELETE", &item.delete),
                ("PATCH", &item.patch),
                ("HEAD", &item.head),
                ("OPTIONS", &item.options),
                ("TRACE", &item.trace),
            ] {
                let op = match op_opt {
                    Some(o) => o,
                    None => continue,
                };
                let mut schemas: HashMap<u16, JSONSchema> = HashMap::new();
                let mut has_default = false;

                for (code, resp_ref) in &op.responses.responses {
                    let resp = match resp_ref {
                        ReferenceOr::Item(r) => r,
                        _ => continue,
                    };
                    let media = match resp.content.get("application/json") {
                        Some(m) => m,
                        None => continue,
                    };
                    let schema_json = match schema_for(media) {
                        Some(j) => j,
                        None => continue,
                    };
                    let compiled = match JSONSchema::compile(&schema_json) {
                        Ok(c) => c,
                        Err(_) => continue,
                    };
                    if let openapiv3::StatusCode::Code(c) = code {
                        schemas.insert(*c, compiled);
                    }
                }

                if let Some(default_ref) = &op.responses.default {
                    if let ReferenceOr::Item(resp) = default_ref {
                        if let Some(media) = resp.content.get("application/json") {
                            if let Some(schema_json) = schema_for(media) {
                                if let Ok(compiled) = JSONSchema::compile(&schema_json) {
                                    schemas.insert(0, compiled);
                                    has_default = true;
                                }
                            }
                        }
                    }
                }

                if !schemas.is_empty() {
                    ops.push(CompiledOp {
                        method: method_name.to_string(),
                        segments: clone_segments(&segments),
                        schemas,
                        has_default,
                    });
                }
            }
        }

        Ok(Self {
            ops,
            buckets: Mutex::new(HashMap::new()),
            bucket_cap: cfg.max_violations_per_minute,
            log_level: LogLevel::parse(&cfg.log_level),
            alert_on_violation: cfg.alert_on_violation,
            violations_total: AtomicU64::new(0),
        })
    }

    fn match_op(&self, method: &str, path: &str) -> Option<&CompiledOp> {
        let req_segs: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
        for op in &self.ops {
            if op.method != method {
                continue;
            }
            if op.segments.len() != req_segs.len() {
                continue;
            }
            let mut ok = true;
            for (s, r) in op.segments.iter().zip(req_segs.iter()) {
                match s {
                    Seg::Literal(lit) => {
                        if lit != r {
                            ok = false;
                            break;
                        }
                    }
                    Seg::Wild => {
                        if r.is_empty() {
                            ok = false;
                            break;
                        }
                    }
                }
            }
            if ok {
                return Some(op);
            }
        }
        None
    }

    /// Validate a parsed JSON response body against the spec.
    pub fn check(&self, method: &str, path: &str, status: u16, body: &serde_json::Value) {
        let op = match self.match_op(method, path) {
            Some(o) => o,
            None => return, // unknown route — nothing to compare against
        };
        let schema = match op.schemas.get(&status) {
            Some(s) => s,
            None if op.has_default => match op.schemas.get(&0) {
                Some(s) => s,
                None => return,
            },
            None => return,
        };

        if let Err(errors) = schema.validate(body) {
            let detail: Vec<String> = errors.take(8).map(|e| format!("{e}")).collect();
            self.record_violation(method, path, status, &detail);
        }
    }

    fn record_violation(&self, method: &str, path: &str, status: u16, detail: &[String]) {
        self.violations_total.fetch_add(1, Ordering::Relaxed);

        // Classify the first error so the metric label is meaningful.
        let kind = classify(detail.first().map(|s| s.as_str()).unwrap_or(""));

        metrics::counter!(
            "lynxx_openapi_drift_total",
            "method" => method.to_string(),
            "path" => path.to_string(),
            "kind" => kind.to_string(),
        )
        .increment(1);

        if !self.alert_on_violation {
            return;
        }

        let key = format!("{method} {path}");
        let mut buckets = self.buckets.lock().unwrap();
        let bucket = buckets
            .entry(key)
            .or_insert_with(|| Bucket::new(self.bucket_cap));
        if !bucket.try_consume() {
            return;
        }

        let summary = detail.join("; ");
        match self.log_level {
            LogLevel::Debug => tracing::debug!(
                method, path, status, kind, errors = %summary,
                "openapi-drift: response schema mismatch",
            ),
            LogLevel::Info => tracing::info!(
                method, path, status, kind, errors = %summary,
                "openapi-drift: response schema mismatch",
            ),
            LogLevel::Warn => tracing::warn!(
                method, path, status, kind, errors = %summary,
                "openapi-drift: response schema mismatch",
            ),
            LogLevel::Error => tracing::error!(
                method, path, status, kind, errors = %summary,
                "openapi-drift: response schema mismatch",
            ),
        }
    }

    /// Catch-all for non-schema failures (non-JSON body when we expected JSON,
    /// etc.). No schema validation, just bump the metric.
    pub fn record(&self, method: &str, path: &str, kind: &str) {
        self.violations_total.fetch_add(1, Ordering::Relaxed);
        metrics::counter!(
            "lynxx_openapi_drift_total",
            "method" => method.to_string(),
            "path" => path.to_string(),
            "kind" => kind.to_string(),
        )
        .increment(1);
    }

    pub fn violations_recorded(&self) -> u64 {
        self.violations_total.load(Ordering::Relaxed)
    }
}

fn clone_segments(segs: &[Seg]) -> Vec<Seg> {
    segs.iter()
        .map(|s| match s {
            Seg::Literal(s) => Seg::Literal(s.clone()),
            Seg::Wild => Seg::Wild,
        })
        .collect()
}

/// Map a `jsonschema` error message string to a coarse Prometheus label.
/// We deliberately keep the cardinality low: three buckets cover the vast
/// majority of violations.
fn classify(err: &str) -> &'static str {
    let lower = err.to_ascii_lowercase();
    if lower.contains("required") || lower.contains("missing") {
        "missing_field"
    } else if lower.contains("additional") || lower.contains("not allowed") {
        "extra_field"
    } else if lower.contains("type") || lower.contains("expected") {
        "type_mismatch"
    } else {
        "other"
    }
}
