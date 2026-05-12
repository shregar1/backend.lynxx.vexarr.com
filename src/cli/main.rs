//! lynxx — declarative CLI for the Lynxx gateway admin API.
//!
//! Talks HTTP to the gateway's admin port (default `http://localhost:9001`).
//! Produces human-friendly tables by default and JSON/YAML on request, plus
//! an `apply`/`dump` pair for idempotent declarative state management,
//! a live `watch`/`tail`, and a `exec` smoke-tester.
//!
//! Aesthetic: monochrome ANSI (bold + dim only) plus light box-drawing
//! borders on TTY tables. Errors go to stderr with a "✗" prefix and a
//! dim "└─" continuation when stdout is a TTY.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io::{self, IsTerminal, Read, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use chrono::{DateTime, Local, Utc};
use clap::{CommandFactory, Parser, Subcommand, ValueEnum};
use clap_complete::{generate, Shell};
use serde::{Deserialize, Serialize};
use serde_json::Value as Json;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// CLI definition
// ---------------------------------------------------------------------------

#[derive(Parser, Debug)]
#[command(
    name = "lynxx",
    version,
    about = "Declarative CLI for the Lynxx gateway",
    long_about = "lynxx is the official admin client for the Lynxx gateway. \
                  It speaks to the gateway's admin HTTP API and offers \
                  imperative CRUD, declarative apply/dump, live watch/tail, \
                  and a built-in route smoke-tester.",
    propagate_version = true
)]
struct Cli {
    /// Admin API base URL (overrides config file)
    #[arg(long, env = "LYNXX_ADMIN", global = true)]
    admin: Option<String>,

    /// Output format for machine-readable commands
    #[arg(long, value_enum, global = true)]
    output: Option<OutputFormat>,

    /// Disable ANSI color output (also honors NO_COLOR env)
    #[arg(long, global = true)]
    no_color: bool,

    /// Show full UUIDs in tables instead of 8-char prefixes
    #[arg(long, global = true)]
    full_id: bool,

    #[command(subcommand)]
    command: Cmd,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
enum OutputFormat {
    Table,
    Json,
    Yaml,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// Quick health probe of the admin API
    Ping,
    /// Dump /status as structured output
    Status,
    /// Manage services
    #[command(subcommand)]
    Services(ServicesCmd),
    /// Manage routes
    #[command(subcommand)]
    Routes(RoutesCmd),
    /// Manage plugins
    #[command(subcommand)]
    Plugins(PluginsCmd),
    /// Install / list / remove WASM plugins from a `.wasm` file
    #[command(subcommand)]
    Wasm(WasmCmd),
    /// Install / list / remove script-runner plugin instances
    #[command(subcommand)]
    Scripts(ScriptsCmd),
    /// Manage consumers
    #[command(subcommand)]
    Consumers(ConsumersCmd),
    /// Manage upstream targets
    #[command(subcommand)]
    Targets(TargetsCmd),
    /// Show per-service circuit/health summary
    Health,
    /// Trigger a config.toml reload on the gateway
    Reload,
    /// Dump full declarative state to stdout (YAML)
    Dump,
    /// Apply a declarative state file (upsert by name, delete missing)
    #[command(
        long_about = "Compute a Kubernetes-style diff plan against the current \
                            gateway state and apply it. Order: services → targets → \
                            routes → plugins, then deletes in reverse. Failures stop \
                            execution; partial application is NOT rolled back."
    )]
    Apply {
        /// Path to YAML state file ("-" for stdin)
        #[arg(short = 'f', long = "file")]
        file: PathBuf,
        /// Print the diff plan but do not mutate (alias of plan-only)
        #[arg(long)]
        dry_run: bool,
        /// Skip the interactive confirmation (CI-friendly)
        #[arg(long)]
        auto_approve: bool,
    },
    /// Import an OpenAPI 3.x spec → services/routes/plugins (kubectl-style diff)
    #[command(
        long_about = "Read an OpenAPI 3.x document (file path or http(s) URL), \
                            translate it into Lynxx services + routes + (optionally) \
                            request-validator and openapi-mock plugins, then run the \
                            same diff/apply pipeline as `lynxx apply`. Re-importing \
                            the same spec is idempotent — names are derived \
                            deterministically from operationId / method+path so the \
                            second run produces an empty plan."
    )]
    Import {
        /// Path to the spec, or an http(s) URL
        spec: String,
        /// Prefix for generated service names (e.g. `--service-prefix=billing` → `billing-listInvoices`)
        #[arg(long)]
        service_prefix: Option<String>,
        /// Override the upstream URL (otherwise taken from spec's `servers[0].url`)
        #[arg(long)]
        upstream: Option<String>,
        /// Strip the spec's basePath off incoming URLs (sets `strip_path=true` on routes)
        #[arg(long)]
        strip_base_path: bool,
        /// Install request-validator plugins per route (default: on)
        #[arg(long, default_value_t = true, action = clap::ArgAction::Set)]
        validators: bool,
        /// Install openapi-mock plugin per route, sourced from spec examples
        #[arg(long)]
        mock: bool,
        /// Print the diff plan but don't apply it
        #[arg(long)]
        dry_run: bool,
        /// Skip the interactive confirmation
        #[arg(long)]
        auto_approve: bool,
    },
    /// Validate a declarative state file without applying
    Validate {
        /// Path to YAML state file ("-" for stdin)
        #[arg(short = 'f', long = "file")]
        file: PathBuf,
    },
    /// Live-tail a list view; redraws every N seconds
    #[command(long_about = "Periodically re-renders the chosen list view in place. \
                            Resources: services, routes, plugins, health. Press Ctrl-C \
                            to exit; the cursor is always restored.")]
    Watch {
        /// Resource to watch
        resource: WatchResource,
        /// Refresh interval in seconds
        #[arg(short = 'n', long, default_value_t = 2)]
        interval: u64,
    },
    /// Live tail of /metrics, deriving per-tick request deltas
    Tail {
        /// Refresh interval in seconds
        #[arg(short = 'n', long, default_value_t = 1)]
        interval: u64,
    },
    /// Smoke-test a route through the gateway proxy
    Exec {
        /// Path on the gateway proxy (e.g. /api/v1/users)
        path: String,
        /// HTTP method
        #[arg(short = 'X', long, default_value = "GET")]
        method: String,
        /// Request body. Prefix with @ to read from a file (e.g. @payload.json)
        #[arg(long)]
        body: Option<String>,
        /// Custom header (repeatable): "Key: Value"
        #[arg(short = 'H', long = "header")]
        headers: Vec<String>,
        /// Override proxy base URL (default: admin host with port - 1)
        #[arg(long)]
        proxy: Option<String>,
        /// Body truncation limit in bytes (0 = no truncation)
        #[arg(long, default_value_t = 4096)]
        max_body: usize,
    },
    /// Browse + replay captured request/response pairs (Time-Travel Debug)
    #[command(subcommand)]
    Record(RecordCmd),
    /// Manage local CLI config (~/.config/lynxx/config.toml)
    #[command(subcommand)]
    Config(ConfigCmd),
    /// Generate shell completion script
    Completion {
        /// Target shell
        shell: Shell,
    },
    /// Run the Lynxx gateway (execs the lynxx-gateway binary in PATH).
    /// All extra arguments are forwarded.
    #[command(trailing_var_arg = true, allow_hyphen_values = true)]
    Start {
        /// Arguments forwarded to lynxx-gateway (e.g. --config /etc/lynxx.toml).
        args: Vec<String>,
    },
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
enum WatchResource {
    Services,
    Routes,
    Plugins,
    Health,
}

#[derive(Subcommand, Debug)]
enum ServicesCmd {
    List,
    Get {
        ident: String,
    },
    Create {
        #[arg(long)]
        name: String,
        #[arg(long)]
        url: String,
        #[arg(long, default_value_t = 3)]
        retries: u32,
        #[arg(long = "timeout", default_value_t = 5000)]
        connect_timeout_ms: u64,
        #[arg(long = "lb")]
        lb_strategy: Option<String>,
    },
    Delete {
        ident: String,
    },
}

#[derive(Subcommand, Debug)]
enum RoutesCmd {
    List,
    Create {
        #[arg(long)]
        service: String,
        /// Comma-separated paths
        #[arg(long, value_delimiter = ',')]
        paths: Vec<String>,
        /// Comma-separated methods
        #[arg(long, value_delimiter = ',')]
        methods: Vec<String>,
        #[arg(long)]
        strip_path: bool,
    },
    Delete {
        id: String,
    },
}

#[derive(Subcommand, Debug)]
enum PluginsCmd {
    List,
    Install {
        /// Plugin name (e.g. jwt, rate-limit)
        name: String,
        /// Attach to a service by name
        #[arg(long)]
        service: Option<String>,
        /// Attach to a route by id
        #[arg(long)]
        route: Option<String>,
        /// JSON config blob
        #[arg(long)]
        config: String,
    },
    Delete {
        id: String,
    },
}

#[derive(Subcommand, Debug)]
enum WasmCmd {
    /// Install a `.wasm` plugin module
    Install {
        /// Path to the `.wasm` file
        path: PathBuf,
        /// Plugin instance name (default: file stem)
        #[arg(long)]
        name: Option<String>,
        /// JSON config blob forwarded to the guest via `lynxx_get_config`
        #[arg(long)]
        config: Option<String>,
        /// Scope to a service by name
        #[arg(long)]
        service: Option<String>,
        /// Scope to a route by id
        #[arg(long)]
        route: Option<String>,
        /// wasmtime fuel budget per request
        #[arg(long, default_value_t = 10_000_000)]
        fuel: u64,
        /// Linear memory cap in kilobytes
        #[arg(long = "memory-kb", default_value_t = 16_384)]
        memory_kb: u32,
        /// Wall-clock budget per request, milliseconds
        #[arg(long = "wall-ms", default_value_t = 50)]
        wall_ms: u64,
    },
    /// List installed plugins whose backing impl is `wasm`
    List,
    /// Remove a wasm plugin instance by id (alias of `plugins delete`)
    Remove {
        /// Plugin instance id
        id: String,
    },
}

#[derive(Subcommand, Debug)]
enum ScriptsCmd {
    /// Install a script-runner plugin instance
    Install {
        /// Command to run pre-request (e.g. `python3`)
        #[arg(long)]
        on_request_cmd: Option<String>,
        /// Arguments for the pre-request command (repeatable)
        #[arg(long = "on-request-args")]
        on_request_args: Vec<String>,
        /// Command to run post-response (e.g. `node`)
        #[arg(long)]
        on_response_cmd: Option<String>,
        /// Arguments for the post-response command (repeatable)
        #[arg(long = "on-response-args")]
        on_response_args: Vec<String>,
        /// Script execution timeout in milliseconds
        #[arg(long, default_value_t = 5000)]
        timeout: u64,
        /// Environment variables (repeatable, KEY=VALUE)
        #[arg(long = "env", value_parser = parse_key_val)]
        env: Vec<(String, String)>,
        /// Max body bytes passed to scripts
        #[arg(long, default_value_t = 2_097_152)]
        max_body_bytes: u64,
        /// Scope to a service by name
        #[arg(long)]
        service: Option<String>,
        /// Scope to a route by id
        #[arg(long)]
        route: Option<String>,
    },
    /// List installed script-runner plugin instances
    List,
    /// Remove a script-runner plugin instance by id
    Delete {
        /// Plugin instance id
        id: String,
    },
}

/// Parse a `KEY=VALUE` pair for `--env`.
fn parse_key_val(s: &str) -> std::result::Result<(String, String), String> {
    let pos = s
        .find('=')
        .ok_or_else(|| format!("invalid KEY=VALUE: no `=` found in `{s}`"))?;
    Ok((s[..pos].to_string(), s[pos + 1..].to_string()))
}

#[derive(Subcommand, Debug)]
enum ConsumersCmd {
    List,
    Create {
        #[arg(long)]
        username: String,
        #[arg(long)]
        custom_id: Option<String>,
    },
    Delete {
        ident: String,
    },
}

#[derive(Subcommand, Debug)]
enum TargetsCmd {
    List {
        #[arg(long)]
        service: Option<String>,
    },
    Add {
        #[arg(long)]
        service: String,
        #[arg(long)]
        url: String,
        #[arg(long, default_value_t = 1)]
        weight: u32,
    },
    Delete {
        id: String,
    },
}

#[derive(Subcommand, Debug)]
enum RecordCmd {
    /// List recent recordings (most recent first)
    List {
        /// Filter to recordings newer than this. Accepts relative
        /// (`30m`, `2h`, `1d`, `7d`) or absolute RFC 3339 timestamps.
        #[arg(long)]
        since: Option<String>,
        /// Filter by HTTP method (case-insensitive)
        #[arg(long)]
        method: Option<String>,
        /// Filter by request path prefix
        #[arg(long = "path-prefix")]
        path_prefix: Option<String>,
        /// Filter by response status code
        #[arg(long)]
        status: Option<u16>,
        /// Cap on returned rows (defaults to 100; max 1000)
        #[arg(long)]
        limit: Option<usize>,
    },
    /// Show the full recording payload (request + response, headers, body)
    Show { id: String },
    /// Replay a recording and diff the response
    Replay {
        id: String,
        /// Replay against this URL. Defaults to the gateway proxy port.
        #[arg(long)]
        against: Option<String>,
    },
    /// Delete a recording (both the on-disk file and the index row)
    Delete { id: String },
}

#[derive(Subcommand, Debug)]
enum ConfigCmd {
    /// Set a config key (e.g. `lynxx config set admin http://host:9001`)
    Set { key: String, value: String },
    /// Get a config key
    Get { key: String },
    /// Show the resolved config file
    Show,
    /// Print the config file path
    Path,
}

// ---------------------------------------------------------------------------
// Wire types — minimal mirror of the admin API surface.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Service {
    id: Uuid,
    name: String,
    url: String,
    retries: u32,
    connect_timeout_ms: u64,
    #[serde(default)]
    lb_strategy: Option<String>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Route {
    id: Uuid,
    paths: Vec<String>,
    methods: Vec<String>,
    service_id: Uuid,
    strip_path: bool,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PluginInstance {
    id: Uuid,
    name: String,
    #[serde(default)]
    service_id: Option<Uuid>,
    #[serde(default)]
    route_id: Option<Uuid>,
    enabled: bool,
    config: Json,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Consumer {
    id: Uuid,
    username: String,
    custom_id: Option<String>,
    api_key: Option<String>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Target {
    id: Uuid,
    service_id: Uuid,
    url: String,
    weight: u32,
    enabled: bool,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

// ── recording wire types ─────────────────────────────────────────────────
//
// Mirrors of `crate::recording::*` shapes — kept here so the CLI binary
// doesn't drag in the gateway crate.

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RecordingIndexRow {
    id: String,
    ts: DateTime<Utc>,
    method: String,
    path: String,
    status: u16,
    duration_ms: u64,
    #[serde(default)]
    service_id: Option<Uuid>,
    #[serde(default)]
    route_id: Option<Uuid>,
    #[serde(default)]
    consumer: Option<String>,
    bytes_request: u64,
    bytes_response: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RecordedRequest {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body_b64: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RecordedResponse {
    status: u16,
    headers: Vec<(String, String)>,
    body_b64: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Recording {
    id: String,
    ts: DateTime<Utc>,
    #[serde(default)]
    service_id: Option<Uuid>,
    #[serde(default)]
    route_id: Option<Uuid>,
    #[serde(default)]
    consumer: Option<String>,
    request: RecordedRequest,
    response: RecordedResponse,
    upstream_url: String,
    duration_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ReplayResult {
    original: Recording,
    new_response: RecordedResponse,
    diff: ResponseDiff,
    elapsed_ms: u64,
    target_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct ResponseDiff {
    #[serde(default)]
    status_changed: Option<(u16, u16)>,
    #[serde(default)]
    headers_added: Vec<(String, String)>,
    #[serde(default)]
    headers_removed: Vec<(String, String)>,
    #[serde(default)]
    headers_changed: Vec<(String, String, String)>,
    #[serde(default)]
    body_diff: BodyDiff,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum BodyDiff {
    Identical,
    BinaryDiff {
        before_bytes: usize,
        after_bytes: usize,
    },
    JsonDiff {
        added: Json,
        removed: Json,
        changed: Json,
    },
    TextDiff {
        lines_changed: u32,
    },
}

impl Default for BodyDiff {
    fn default() -> Self {
        BodyDiff::Identical
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StatusResponse {
    status: String,
    version: String,
    uptime_seconds: i64,
    services_count: usize,
    routes_count: usize,
    plugins_count: usize,
    consumers_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ServiceHealth {
    service_id: Uuid,
    status: String,
    circuit: String,
    consecutive_failures: u32,
    last_check_at: Option<DateTime<Utc>>,
    last_error: Option<String>,
}

// ---------------------------------------------------------------------------
// Declarative schema (dump / apply)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct DeclarativeState {
    #[serde(default)]
    services: Vec<DeclService>,
    #[serde(default)]
    routes: Vec<DeclRoute>,
    #[serde(default)]
    plugins: Vec<DeclPlugin>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct DeclService {
    name: String,
    url: String,
    #[serde(default = "default_retries")]
    retries: u32,
    #[serde(default = "default_timeout")]
    connect_timeout_ms: u64,
    #[serde(default = "default_lb")]
    lb_strategy: String,
    #[serde(default)]
    targets: Vec<DeclTarget>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct DeclTarget {
    url: String,
    #[serde(default = "default_weight")]
    weight: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct DeclRoute {
    service: String,
    paths: Vec<String>,
    #[serde(default)]
    methods: Vec<String>,
    #[serde(default)]
    strip_path: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct DeclPlugin {
    name: String,
    /// One of: "global", or attached to a named service / route.
    #[serde(default = "default_scope")]
    scope: String,
    #[serde(default)]
    service: Option<String>,
    /// Route reference: paths joined by ";" — must match an existing route's paths set.
    #[serde(default)]
    route: Option<String>,
    #[serde(default)]
    config: Json,
}

fn default_retries() -> u32 {
    3
}
fn default_timeout() -> u64 {
    5000
}
fn default_weight() -> u32 {
    1
}
fn default_lb() -> String {
    "round-robin".into()
}
fn default_scope() -> String {
    "global".into()
}

/// Inline catalog for `validate`. Mirrors the gateway's built-in plugins.
const PLUGIN_CATALOG: &[&str] = &[
    "jwt",
    "key-auth",
    "basic-auth",
    "hmac-auth",
    "rate-limit",
    "rate-limiting",
    "cors",
    "ip-restriction",
    "request-transformer",
    "response-transformer",
    "request-size-limit",
    "prometheus",
    "logging",
    "correlation-id",
    "acl",
    "oauth2",
];

// ---------------------------------------------------------------------------
// Color / TTY plumbing.
// ---------------------------------------------------------------------------

static COLOR_OFF: AtomicBool = AtomicBool::new(false);
static FULL_ID: AtomicBool = AtomicBool::new(false);

fn color_enabled() -> bool {
    !COLOR_OFF.load(Ordering::Relaxed) && io::stdout().is_terminal()
}
fn color_enabled_stderr() -> bool {
    !COLOR_OFF.load(Ordering::Relaxed) && io::stderr().is_terminal()
}

fn bold(s: &str) -> String {
    if color_enabled() {
        format!("\x1b[1m{s}\x1b[0m")
    } else {
        s.to_string()
    }
}
fn dim(s: &str) -> String {
    if color_enabled() {
        format!("\x1b[2m{s}\x1b[0m")
    } else {
        s.to_string()
    }
}
fn red(s: &str) -> String {
    if color_enabled_stderr() {
        format!("\x1b[31m{s}\x1b[0m")
    } else {
        s.to_string()
    }
}
fn green(s: &str) -> String {
    if color_enabled() {
        format!("\x1b[32m{s}\x1b[0m")
    } else {
        s.to_string()
    }
}
fn yellow(s: &str) -> String {
    if color_enabled() {
        format!("\x1b[33m{s}\x1b[0m")
    } else {
        s.to_string()
    }
}

// ---------------------------------------------------------------------------
// HTTP client. Surfaces JSON `{error: "…"}` envelopes.
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct ApiError {
    method: &'static str,
    url: String,
    status: reqwest::StatusCode,
    body: String,
    /// Best-effort parse of `{error: "..."}` or `{message: "..."}`.
    message: Option<String>,
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let msg = self.message.as_deref().unwrap_or_else(|| {
            if self.body.is_empty() {
                self.status.canonical_reason().unwrap_or("error")
            } else {
                self.body.as_str()
            }
        });
        write!(
            f,
            "{} {} → {}: {}",
            self.method,
            self.url,
            self.status,
            msg.trim()
        )
    }
}
impl std::error::Error for ApiError {}

fn parse_error_envelope(body: &str) -> Option<String> {
    let v: Json = serde_json::from_str(body).ok()?;
    if let Some(s) = v.get("error").and_then(|x| x.as_str()) {
        return Some(s.into());
    }
    if let Some(s) = v.get("message").and_then(|x| x.as_str()) {
        return Some(s.into());
    }
    None
}

struct Client {
    base: String,
    http: reqwest::Client,
}

impl Client {
    fn new(base: impl Into<String>) -> Self {
        Self {
            base: base.into().trim_end_matches('/').to_string(),
            http: reqwest::Client::new(),
        }
    }

    async fn get<T: for<'de> Deserialize<'de>>(&self, path: &str) -> Result<T> {
        let url = format!("{}{}", self.base, path);
        let resp = self
            .http
            .get(&url)
            .send()
            .await
            .with_context(|| format!("GET {url}"))?;
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(ApiError {
                method: "GET",
                url,
                status,
                message: parse_error_envelope(&text),
                body: text,
            }
            .into());
        }
        serde_json::from_str(&text).with_context(|| format!("decoding response from {url}"))
    }

    async fn get_text(&self, path: &str) -> Result<String> {
        let url = format!("{}{}", self.base, path);
        let resp = self
            .http
            .get(&url)
            .send()
            .await
            .with_context(|| format!("GET {url}"))?;
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(ApiError {
                method: "GET",
                url,
                status,
                message: parse_error_envelope(&text),
                body: text,
            }
            .into());
        }
        Ok(text)
    }

    async fn post<B: Serialize, T: for<'de> Deserialize<'de>>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<T> {
        let url = format!("{}{}", self.base, path);
        let resp = self
            .http
            .post(&url)
            .json(body)
            .send()
            .await
            .with_context(|| format!("POST {url}"))?;
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(ApiError {
                method: "POST",
                url,
                status,
                message: parse_error_envelope(&text),
                body: text,
            }
            .into());
        }
        serde_json::from_str(&text).with_context(|| format!("decoding response from {url}"))
    }

    async fn post_empty(&self, path: &str) -> Result<Json> {
        let url = format!("{}{}", self.base, path);
        let resp = self
            .http
            .post(&url)
            .send()
            .await
            .with_context(|| format!("POST {url}"))?;
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(ApiError {
                method: "POST",
                url,
                status,
                message: parse_error_envelope(&text),
                body: text,
            }
            .into());
        }
        if text.is_empty() {
            return Ok(Json::Null);
        }
        Ok(serde_json::from_str(&text).unwrap_or(Json::String(text)))
    }

    async fn delete(&self, path: &str) -> Result<()> {
        let url = format!("{}{}", self.base, path);
        let resp = self
            .http
            .delete(&url)
            .send()
            .await
            .with_context(|| format!("DELETE {url}"))?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(ApiError {
                method: "DELETE",
                url,
                status,
                message: parse_error_envelope(&text),
                body: text,
            }
            .into());
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Output helpers.
// ---------------------------------------------------------------------------

/// Print a monospace table. On TTY, uses light box-drawing chars (dim).
/// On non-TTY, falls back to plain pipe separators.
fn print_table(headers: &[&str], rows: &[Vec<String>]) {
    if rows.is_empty() {
        eprintln!("{}", dim("(no rows)"));
        return;
    }
    let ncol = headers.len();
    let mut widths: Vec<usize> = headers.iter().map(|h| h.len()).collect();
    for row in rows {
        for (i, cell) in row.iter().enumerate().take(ncol) {
            if cell.len() > widths[i] {
                widths[i] = cell.len();
            }
        }
    }

    let tty = color_enabled();
    let (h, v, tl, tr, bl, br, mt, mb, ml, mr, x) = if tty {
        ("─", "│", "┌", "┐", "└", "┘", "┬", "┴", "├", "┤", "┼")
    } else {
        ("-", "|", "+", "+", "+", "+", "+", "+", "+", "+", "+")
    };

    // Build a horizontal rule with given junctions.
    let rule = |left: &str, mid: &str, right: &str| {
        let mut out = String::from(left);
        for (i, w) in widths.iter().enumerate() {
            out.push_str(&h.repeat(w + 2));
            out.push_str(if i + 1 < ncol { mid } else { right });
        }
        out
    };

    println!("{}", dim(&rule(tl, mt, tr)));
    // Header row.
    let mut header_line = String::new();
    header_line.push_str(&dim(v));
    for (i, hd) in headers.iter().enumerate() {
        let pad = " ".repeat(widths[i].saturating_sub(hd.len()));
        header_line.push_str(&format!(" {}{} ", bold(hd), pad));
        header_line.push_str(&dim(v));
        let _ = i;
    }
    println!("{header_line}");
    println!("{}", dim(&rule(ml, x, mr)));
    // Body rows.
    for row in rows {
        let mut line = String::new();
        line.push_str(&dim(v));
        for (i, cell) in row.iter().enumerate().take(ncol) {
            let pad = " ".repeat(widths[i].saturating_sub(cell.len()));
            line.push_str(&format!(" {cell}{pad} "));
            line.push_str(&dim(v));
        }
        println!("{line}");
    }
    println!("{}", dim(&rule(bl, mb, br)));
}

/// Emit a value in the requested format. `Table` falls back to JSON.
fn emit<T: Serialize>(v: &T, fmt: OutputFormat) -> Result<()> {
    match fmt {
        OutputFormat::Json | OutputFormat::Table => {
            println!("{}", serde_json::to_string_pretty(v)?);
        }
        OutputFormat::Yaml => {
            print!("{}", serde_yaml::to_string(v)?);
        }
    }
    Ok(())
}

fn fmt_id(id: &Uuid) -> String {
    if FULL_ID.load(Ordering::Relaxed) {
        id.to_string()
    } else {
        id.to_string()[..8].to_string()
    }
}

// ---------------------------------------------------------------------------
// Levenshtein for "did you mean?" hints.
// ---------------------------------------------------------------------------

fn levenshtein(a: &str, b: &str) -> usize {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    let (m, n) = (a.len(), b.len());
    if m == 0 {
        return n;
    }
    if n == 0 {
        return m;
    }
    let mut prev: Vec<usize> = (0..=n).collect();
    let mut cur = vec![0usize; n + 1];
    for i in 1..=m {
        cur[0] = i;
        for j in 1..=n {
            let cost = if a[i - 1] == b[j - 1] { 0 } else { 1 };
            cur[j] = (prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + cost);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[n]
}

fn fuzzy_suggest<'a>(needle: &str, haystack: impl IntoIterator<Item = &'a str>) -> Option<String> {
    let n_lower = needle.to_lowercase();
    let mut best: Option<(usize, &str)> = None;
    for cand in haystack {
        let d = levenshtein(&n_lower, &cand.to_lowercase());
        let threshold = (cand.len() / 3).max(2);
        if d <= threshold && best.map_or(true, |(bd, _)| d < bd) {
            best = Some((d, cand));
        }
    }
    best.map(|(_, s)| s.to_string())
}

// ---------------------------------------------------------------------------
// Resolve helpers — accept name OR id for human convenience, with suggestions.
// ---------------------------------------------------------------------------

async fn resolve_service(client: &Client, ident: &str) -> Result<Service> {
    let services: Vec<Service> = client.get("/services").await?;
    if let Ok(uuid) = Uuid::parse_str(ident) {
        if let Some(s) = services.iter().find(|s| s.id == uuid) {
            return Ok(s.clone());
        }
    }
    if let Some(s) = services.iter().find(|s| s.name == ident) {
        return Ok(s.clone());
    }
    let matches: Vec<&Service> = services
        .iter()
        .filter(|s| s.id.to_string().starts_with(ident))
        .collect();
    match matches.len() {
        1 => Ok(matches[0].clone()),
        0 => {
            let suggest = fuzzy_suggest(ident, services.iter().map(|s| s.name.as_str()));
            match suggest {
                Some(s) => Err(anyhow!("no service matched '{ident}'. Did you mean: {s}?")),
                None => Err(anyhow!("no service matched '{ident}'")),
            }
        }
        _ => Err(anyhow!(
            "'{ident}' is ambiguous ({} matches)",
            matches.len()
        )),
    }
}

async fn resolve_consumer(client: &Client, ident: &str) -> Result<Consumer> {
    let consumers: Vec<Consumer> = client.get("/consumers").await?;
    if let Ok(uuid) = Uuid::parse_str(ident) {
        if let Some(c) = consumers.iter().find(|c| c.id == uuid) {
            return Ok(c.clone());
        }
    }
    if let Some(c) = consumers.iter().find(|c| c.username == ident) {
        return Ok(c.clone());
    }
    let suggest = fuzzy_suggest(ident, consumers.iter().map(|c| c.username.as_str()));
    match suggest {
        Some(s) => Err(anyhow!("no consumer matched '{ident}'. Did you mean: {s}?")),
        None => Err(anyhow!("no consumer matched '{ident}'")),
    }
}

// ---------------------------------------------------------------------------
// Local config (~/.config/lynxx/config.toml)
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Serialize, Deserialize)]
struct LocalConfig {
    #[serde(default)]
    admin: Option<String>,
    #[serde(default)]
    output: Option<String>,
}

fn config_path() -> Result<PathBuf> {
    let base = dirs::config_dir().ok_or_else(|| anyhow!("could not determine user config dir"))?;
    Ok(base.join("lynxx").join("config.toml"))
}

fn load_config() -> LocalConfig {
    let path = match config_path() {
        Ok(p) => p,
        Err(_) => return LocalConfig::default(),
    };
    let raw = match fs::read_to_string(&path) {
        Ok(s) => s,
        Err(_) => return LocalConfig::default(),
    };
    toml::from_str(&raw).unwrap_or_default()
}

fn save_config(cfg: &LocalConfig) -> Result<PathBuf> {
    let path = config_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    let raw = toml::to_string_pretty(cfg)?;
    fs::write(&path, raw).with_context(|| format!("writing {}", path.display()))?;
    Ok(path)
}

fn cmd_config(action: ConfigCmd) -> Result<()> {
    match action {
        ConfigCmd::Path => {
            println!("{}", config_path()?.display());
        }
        ConfigCmd::Show => {
            let path = config_path()?;
            match fs::read_to_string(&path) {
                Ok(raw) => print!("{raw}"),
                Err(_) => println!("{}", dim(&format!("(no config at {})", path.display()))),
            }
        }
        ConfigCmd::Get { key } => {
            let cfg = load_config();
            let val = match key.as_str() {
                "admin" => cfg.admin,
                "output" => cfg.output,
                other => bail!("unknown config key '{other}' (valid: admin, output)"),
            };
            match val {
                Some(v) => println!("{v}"),
                None => println!("{}", dim("(unset)")),
            }
        }
        ConfigCmd::Set { key, value } => {
            let mut cfg = load_config();
            match key.as_str() {
                "admin" => cfg.admin = Some(value.clone()),
                "output" => {
                    if !["table", "json", "yaml"].contains(&value.as_str()) {
                        bail!("output must be one of: table, json, yaml");
                    }
                    cfg.output = Some(value.clone());
                }
                other => bail!("unknown config key '{other}' (valid: admin, output)"),
            }
            let path = save_config(&cfg)?;
            println!(
                "{} set {}={} ({})",
                bold("✓"),
                key,
                value,
                dim(&path.display().to_string())
            );
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Existing commands (with FULL_ID-aware id formatting).
// ---------------------------------------------------------------------------

async fn cmd_ping(client: &Client) -> Result<()> {
    let s: StatusResponse = client.get("/status").await?;
    println!(
        "{} lynxx {} — {} (up {})",
        green("✓"),
        s.version,
        s.status,
        humanize_secs(s.uptime_seconds)
    );
    println!(
        "  {} services · {} routes · {} plugins · {} consumers",
        s.services_count, s.routes_count, s.plugins_count, s.consumers_count,
    );
    Ok(())
}

async fn cmd_status(client: &Client, fmt: OutputFormat) -> Result<()> {
    if fmt == OutputFormat::Table {
        print_banner();
    }
    let s: StatusResponse = client.get("/status").await?;
    emit(&s, fmt)
}

fn print_banner() {
    if !color_enabled_stderr() {
        eprintln!("\n  L Y N X X\n");
        return;
    }
    const ROWS: [&str; 7] = [
        "##       ##    ## ##    ## ##     ## ##     ## ",
        "##        ##  ##  ###   ##  ##   ##   ##   ##  ",
        "##         ####   ####  ##   ## ##     ## ##   ",
        "##          ##    ## ## ##    ###       ###    ",
        "##          ##    ##  ####   ## ##     ## ##   ",
        "##          ##    ##   ###  ##   ##   ##   ##  ",
        "########    ##    ##    ## ##     ## ##     ## ",
    ];
    let fg = "\x1b[97;1m";
    let bg = "\x1b[38;5;236m";
    let red = "\x1b[1;38;2;198;45;49m";
    let reset = "\x1b[0m";
    let mut buf = String::with_capacity(2048);
    buf.push('\n');
    for row in &ROWS {
        buf.push_str("  ");
        for (i, ch) in row.chars().enumerate() {
            if ch == '#' {
                buf.push_str(if i >= 37 { red } else { fg });
            } else {
                buf.push_str(bg);
            }
            buf.push('\u{2588}');
        }
        buf.push_str(reset);
        buf.push('\n');
    }
    buf.push('\n');
    eprint!("{buf}");
}

fn humanize_secs(s: i64) -> String {
    if s < 60 {
        format!("{s}s")
    } else if s < 3600 {
        format!("{}m{}s", s / 60, s % 60)
    } else if s < 86_400 {
        format!("{}h{}m", s / 3600, (s % 3600) / 60)
    } else {
        format!("{}d{}h", s / 86_400, (s % 86_400) / 3600)
    }
}

async fn cmd_services_list(client: &Client, fmt: OutputFormat) -> Result<()> {
    let services: Vec<Service> = client.get("/services").await?;
    if fmt != OutputFormat::Table {
        return emit(&services, fmt);
    }
    let rows: Vec<Vec<String>> = services
        .iter()
        .map(|s| {
            vec![
                fmt_id(&s.id),
                s.name.clone(),
                s.url.clone(),
                s.retries.to_string(),
                format!("{}ms", s.connect_timeout_ms),
                s.lb_strategy
                    .clone()
                    .unwrap_or_else(|| "round-robin".into()),
            ]
        })
        .collect();
    print_table(&["ID", "NAME", "URL", "RETRIES", "TIMEOUT", "LB"], &rows);
    Ok(())
}

async fn cmd_services_get(client: &Client, ident: &str, fmt: OutputFormat) -> Result<()> {
    let svc = resolve_service(client, ident).await?;
    emit(&svc, fmt)
}

async fn cmd_services_create(
    client: &Client,
    name: String,
    url: String,
    retries: u32,
    connect_timeout_ms: u64,
    lb_strategy: Option<String>,
    fmt: OutputFormat,
) -> Result<()> {
    let mut body = serde_json::json!({
        "name": name, "url": url,
        "retries": retries, "connect_timeout_ms": connect_timeout_ms,
    });
    if let Some(lb) = lb_strategy {
        body["lb_strategy"] = Json::String(lb);
    }
    let svc: Service = client.post("/services", &body).await?;
    if fmt == OutputFormat::Table {
        println!(
            "{} created service {} ({})",
            green("✓"),
            svc.name,
            fmt_id(&svc.id)
        );
    } else {
        emit(&svc, fmt)?;
    }
    Ok(())
}

async fn cmd_services_delete(client: &Client, ident: &str) -> Result<()> {
    let svc = resolve_service(client, ident).await?;
    client.delete(&format!("/services/{}", svc.id)).await?;
    println!(
        "{} deleted service {} ({})",
        green("✓"),
        svc.name,
        fmt_id(&svc.id)
    );
    Ok(())
}

async fn cmd_routes_list(client: &Client, fmt: OutputFormat) -> Result<()> {
    let routes: Vec<Route> = client.get("/routes").await?;
    if fmt != OutputFormat::Table {
        return emit(&routes, fmt);
    }
    let services: Vec<Service> = client.get("/services").await.unwrap_or_default();
    let by_id: HashMap<Uuid, String> = services.iter().map(|s| (s.id, s.name.clone())).collect();
    let rows: Vec<Vec<String>> = routes
        .iter()
        .map(|r| {
            vec![
                fmt_id(&r.id),
                by_id
                    .get(&r.service_id)
                    .cloned()
                    .unwrap_or_else(|| fmt_id(&r.service_id)),
                r.paths.join(","),
                if r.methods.is_empty() {
                    "*".into()
                } else {
                    r.methods.join(",")
                },
                if r.strip_path {
                    "yes".into()
                } else {
                    "no".into()
                },
            ]
        })
        .collect();
    print_table(&["ID", "SERVICE", "PATHS", "METHODS", "STRIP"], &rows);
    Ok(())
}

async fn cmd_routes_create(
    client: &Client,
    service: String,
    paths: Vec<String>,
    methods: Vec<String>,
    strip_path: bool,
    fmt: OutputFormat,
) -> Result<()> {
    let svc = resolve_service(client, &service).await?;
    let body = serde_json::json!({
        "paths": paths, "methods": methods,
        "service_id": svc.id, "strip_path": strip_path,
    });
    let route: Route = client.post("/routes", &body).await?;
    if fmt == OutputFormat::Table {
        println!(
            "{} created route {} → {} (paths: {})",
            green("✓"),
            fmt_id(&route.id),
            svc.name,
            route.paths.join(",")
        );
    } else {
        emit(&route, fmt)?;
    }
    Ok(())
}

async fn cmd_routes_delete(client: &Client, id: &str) -> Result<()> {
    let uuid = Uuid::parse_str(id).context("route id must be a UUID")?;
    client.delete(&format!("/routes/{uuid}")).await?;
    println!("{} deleted route {}", green("✓"), fmt_id(&uuid));
    Ok(())
}

async fn cmd_plugins_list(client: &Client, fmt: OutputFormat) -> Result<()> {
    let plugins: Vec<PluginInstance> = client.get("/plugins").await?;
    if fmt != OutputFormat::Table {
        return emit(&plugins, fmt);
    }
    let services: Vec<Service> = client.get("/services").await.unwrap_or_default();
    let by_id: HashMap<Uuid, String> = services.iter().map(|s| (s.id, s.name.clone())).collect();
    let rows: Vec<Vec<String>> = plugins
        .iter()
        .map(|p| {
            let scope = match (p.service_id, p.route_id) {
                (Some(s), _) => format!(
                    "service:{}",
                    by_id.get(&s).cloned().unwrap_or_else(|| fmt_id(&s))
                ),
                (_, Some(r)) => format!("route:{}", fmt_id(&r)),
                _ => "global".into(),
            };
            vec![
                fmt_id(&p.id),
                p.name.clone(),
                scope,
                if p.enabled { "yes".into() } else { "no".into() },
            ]
        })
        .collect();
    print_table(&["ID", "NAME", "SCOPE", "ENABLED"], &rows);
    Ok(())
}

async fn cmd_plugins_install(
    client: &Client,
    name: String,
    service: Option<String>,
    route: Option<String>,
    config: String,
    fmt: OutputFormat,
) -> Result<()> {
    let cfg: Json =
        serde_json::from_str(&config).context("--config must be a valid JSON object")?;
    let service_id = if let Some(s) = service {
        Some(resolve_service(client, &s).await?.id)
    } else {
        None
    };
    let route_id = match route {
        Some(r) => Some(Uuid::parse_str(&r).context("--route must be a UUID")?),
        None => None,
    };
    let body = serde_json::json!({
        "name": name, "service_id": service_id, "route_id": route_id,
        "enabled": true, "config": cfg,
    });
    let plugin: PluginInstance = client.post("/plugins", &body).await?;
    if fmt == OutputFormat::Table {
        println!(
            "{} installed plugin {} ({})",
            green("✓"),
            plugin.name,
            fmt_id(&plugin.id)
        );
    } else {
        emit(&plugin, fmt)?;
    }
    Ok(())
}

async fn cmd_plugins_delete(client: &Client, id: &str) -> Result<()> {
    let uuid = Uuid::parse_str(id).context("plugin id must be a UUID")?;
    client.delete(&format!("/plugins/{uuid}")).await?;
    println!("{} deleted plugin {}", green("✓"), fmt_id(&uuid));
    Ok(())
}

// ── lynxx wasm ────────────────────────────────────────────────────────────
//
// Convenience wrapper over the existing `/plugins` endpoints for the
// "I have a .wasm and I want it on the gateway" flow. Reads the file,
// base64-encodes, packs the runtime knobs (fuel / memory / wall-time)
// and the optional user config into the `wasm` plugin's config schema,
// and POSTs.

const WASM_MAX_BYTES: u64 = 5 * 1024 * 1024;

async fn cmd_wasm_install(
    client: &Client,
    path: PathBuf,
    name: Option<String>,
    config: Option<String>,
    service: Option<String>,
    route: Option<String>,
    fuel: u64,
    memory_kb: u32,
    wall_ms: u64,
    fmt: OutputFormat,
) -> Result<()> {
    use base64::Engine as _;

    let meta = fs::metadata(&path).with_context(|| format!("read {}", path.display()))?;
    if meta.len() > WASM_MAX_BYTES {
        bail!(
            "wasm module is {} bytes; gateway hard-limit is {} (5MB)",
            meta.len(),
            WASM_MAX_BYTES
        );
    }
    let bytes = fs::read(&path).with_context(|| format!("read {}", path.display()))?;
    if !bytes.starts_with(b"\0asm") {
        bail!(
            "{} is not a WebAssembly module (missing \\0asm magic)",
            path.display()
        );
    }

    let display_name = name.unwrap_or_else(|| {
        path.file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("wasm-plugin")
            .to_string()
    });

    let user_config: Option<Json> = match config {
        Some(s) => Some(serde_json::from_str(&s).context("--config must be valid JSON")?),
        None => None,
    };

    let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
    let mut wasm_config = serde_json::json!({
        "module_b64": b64,
        "fuel_per_request": fuel,
        "memory_max_kb": memory_kb,
        "wall_timeout_ms": wall_ms,
    });
    if let Some(uc) = user_config {
        wasm_config["user_config"] = uc;
    }

    let service_id = if let Some(s) = service {
        Some(resolve_service(client, &s).await?.id)
    } else {
        None
    };
    let route_id = match route {
        Some(r) => Some(Uuid::parse_str(&r).context("--route must be a UUID")?),
        None => None,
    };

    let body = serde_json::json!({
        "name": "wasm",
        "service_id": service_id,
        "route_id": route_id,
        "enabled": true,
        "config": wasm_config,
    });
    let plugin: PluginInstance = client.post("/plugins", &body).await?;
    if fmt == OutputFormat::Table {
        println!(
            "{} installed wasm plugin {} ({}, {} bytes from {})",
            green("✓"),
            display_name,
            fmt_id(&plugin.id),
            bytes.len(),
            path.display(),
        );
    } else {
        emit(&plugin, fmt)?;
    }
    Ok(())
}

async fn cmd_wasm_list(client: &Client, fmt: OutputFormat) -> Result<()> {
    let plugins: Vec<PluginInstance> = client.get("/plugins").await?;
    let wasm_only: Vec<PluginInstance> = plugins.into_iter().filter(|p| p.name == "wasm").collect();
    if fmt != OutputFormat::Table {
        return emit(&wasm_only, fmt);
    }
    let services: Vec<Service> = client.get("/services").await.unwrap_or_default();
    let by_id: HashMap<Uuid, String> = services.iter().map(|s| (s.id, s.name.clone())).collect();
    let rows: Vec<Vec<String>> = wasm_only
        .iter()
        .map(|p| {
            let scope = match (p.service_id, p.route_id) {
                (Some(s), _) => format!(
                    "service:{}",
                    by_id.get(&s).cloned().unwrap_or_else(|| fmt_id(&s))
                ),
                (_, Some(r)) => format!("route:{}", fmt_id(&r)),
                _ => "global".into(),
            };
            vec![
                fmt_id(&p.id),
                scope,
                if p.enabled { "yes".into() } else { "no".into() },
            ]
        })
        .collect();
    print_table(&["ID", "SCOPE", "ENABLED"], &rows);
    Ok(())
}

async fn cmd_wasm_remove(client: &Client, id: &str) -> Result<()> {
    cmd_plugins_delete(client, id).await
}

// ── lynxx scripts ─────────────────────────────────────────────────────────
//
// Typed CLI for the `script-runner` plugin. Builds the expected JSON config
// from structured flags so the user doesn't have to hand-craft JSON blobs.

#[allow(clippy::too_many_arguments)]
async fn cmd_scripts_install(
    client: &Client,
    on_request_cmd: Option<String>,
    on_request_args: Vec<String>,
    on_response_cmd: Option<String>,
    on_response_args: Vec<String>,
    timeout: u64,
    env: Vec<(String, String)>,
    max_body_bytes: u64,
    service: Option<String>,
    route: Option<String>,
    fmt: OutputFormat,
) -> Result<()> {
    if on_request_cmd.is_none() && on_response_cmd.is_none() {
        bail!("at least one of --on-request-cmd or --on-response-cmd is required");
    }

    let mut config = serde_json::json!({
        "max_body_bytes": max_body_bytes,
    });

    if let Some(cmd) = &on_request_cmd {
        config["on_request"] = serde_json::json!({
            "command": cmd,
            "args": on_request_args,
            "timeout_ms": timeout,
        });
    }

    if let Some(cmd) = &on_response_cmd {
        config["on_response"] = serde_json::json!({
            "command": cmd,
            "args": on_response_args,
            "timeout_ms": timeout,
        });
    }

    if !env.is_empty() {
        let env_map: serde_json::Map<String, Json> = env
            .into_iter()
            .map(|(k, v)| (k, Json::String(v)))
            .collect();
        config["env"] = Json::Object(env_map);
    }

    let service_id = if let Some(s) = service {
        Some(resolve_service(client, &s).await?.id)
    } else {
        None
    };
    let route_id = match route {
        Some(r) => Some(Uuid::parse_str(&r).context("--route must be a UUID")?),
        None => None,
    };

    let body = serde_json::json!({
        "name": "script-runner",
        "service_id": service_id,
        "route_id": route_id,
        "enabled": true,
        "config": config,
    });

    let plugin: PluginInstance = client.post("/plugins", &body).await?;
    if fmt == OutputFormat::Table {
        let hooks: Vec<&str> = [
            on_request_cmd.as_deref().map(|_| "on_request"),
            on_response_cmd.as_deref().map(|_| "on_response"),
        ]
        .into_iter()
        .flatten()
        .collect();
        println!(
            "{} installed script-runner plugin ({}, hooks: {})",
            green("✓"),
            fmt_id(&plugin.id),
            hooks.join("+"),
        );
    } else {
        emit(&plugin, fmt)?;
    }
    Ok(())
}

async fn cmd_scripts_list(client: &Client, fmt: OutputFormat) -> Result<()> {
    let plugins: Vec<PluginInstance> = client.get("/plugins").await?;
    let scripts_only: Vec<PluginInstance> = plugins
        .into_iter()
        .filter(|p| p.name == "script-runner")
        .collect();
    if fmt != OutputFormat::Table {
        return emit(&scripts_only, fmt);
    }
    let services: Vec<Service> = client.get("/services").await.unwrap_or_default();
    let by_id: HashMap<Uuid, String> = services.iter().map(|s| (s.id, s.name.clone())).collect();
    let rows: Vec<Vec<String>> = scripts_only
        .iter()
        .map(|p| {
            let scope = match (p.service_id, p.route_id) {
                (Some(s), _) => format!(
                    "service:{}",
                    by_id.get(&s).cloned().unwrap_or_else(|| fmt_id(&s))
                ),
                (_, Some(r)) => format!("route:{}", fmt_id(&r)),
                _ => "global".into(),
            };
            let hooks = {
                let mut h = Vec::new();
                if p.config.get("on_request").is_some() {
                    h.push("req");
                }
                if p.config.get("on_response").is_some() {
                    h.push("res");
                }
                if h.is_empty() {
                    "none".to_string()
                } else {
                    h.join("+")
                }
            };
            vec![
                fmt_id(&p.id),
                scope,
                hooks,
                if p.enabled { "yes".into() } else { "no".into() },
            ]
        })
        .collect();
    print_table(&["ID", "SCOPE", "HOOKS", "ENABLED"], &rows);
    Ok(())
}

async fn cmd_scripts_delete(client: &Client, id: &str) -> Result<()> {
    cmd_plugins_delete(client, id).await
}

// ── lynxx record ──────────────────────────────────────────────────────────
//
// Browse/replay/delete request recordings via the admin API. The diff
// rendering for `replay` mirrors the BodyDiff variants returned by the
// gateway: Identical → tick, JsonDiff → +/-/~ tree, TextDiff → line count,
// BinaryDiff → byte-size pair.

/// Parse a `--since` value: relative shorthand (`30m`, `2h`, `1d`, `7d`)
/// or absolute RFC 3339. Empty → None.
fn parse_since(raw: &str) -> Result<DateTime<Utc>> {
    let s = raw.trim();
    if s.is_empty() {
        bail!("--since: empty string");
    }
    // Try RFC 3339 first.
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return Ok(dt.with_timezone(&Utc));
    }
    // Relative: integer + unit.
    let (num_part, unit_part) =
        s.split_at(s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len()));
    let n: i64 = num_part
        .parse()
        .with_context(|| format!("--since: not a number: '{num_part}'"))?;
    let dur = match unit_part {
        "s" | "sec" | "secs" => chrono::Duration::seconds(n),
        "m" | "min" | "mins" => chrono::Duration::minutes(n),
        "h" | "hr" | "hrs" => chrono::Duration::hours(n),
        "d" | "day" | "days" => chrono::Duration::days(n),
        other => bail!("--since: unknown unit '{other}' (use s/m/h/d or RFC 3339)"),
    };
    Ok(Utc::now() - dur)
}

fn humanize_relative(dt: DateTime<Utc>) -> String {
    let delta = Utc::now().signed_duration_since(dt);
    let secs = delta.num_seconds();
    if secs < 0 {
        return "in future".into();
    }
    if secs < 60 {
        format!("{secs}s ago")
    } else if secs < 3600 {
        format!("{}m ago", secs / 60)
    } else if secs < 86_400 {
        format!("{}h ago", secs / 3600)
    } else {
        format!("{}d ago", secs / 86_400)
    }
}

fn rec_short_id(id: &str) -> String {
    if FULL_ID.load(Ordering::Relaxed) {
        id.to_string()
    } else {
        id.chars().take(8).collect()
    }
}

async fn cmd_record_list(
    client: &Client,
    since: Option<String>,
    method: Option<String>,
    path_prefix: Option<String>,
    status: Option<u16>,
    limit: Option<usize>,
    fmt: OutputFormat,
) -> Result<()> {
    let mut qs: Vec<String> = Vec::new();
    if let Some(s) = since.as_deref() {
        let dt = parse_since(s)?;
        // Encode as RFC 3339 — what the admin API expects.
        qs.push(format!("since={}", urlencode(&dt.to_rfc3339())));
    }
    if let Some(m) = method.as_deref() {
        qs.push(format!("method={}", urlencode(m)));
    }
    if let Some(p) = path_prefix.as_deref() {
        qs.push(format!("path_prefix={}", urlencode(p)));
    }
    if let Some(s) = status {
        qs.push(format!("status={s}"));
    }
    if let Some(l) = limit {
        qs.push(format!("limit={l}"));
    }
    let path = if qs.is_empty() {
        "/recordings".to_string()
    } else {
        format!("/recordings?{}", qs.join("&"))
    };
    let rows: Vec<RecordingIndexRow> = client.get(&path).await?;
    if fmt != OutputFormat::Table {
        return emit(&rows, fmt);
    }
    let table_rows: Vec<Vec<String>> = rows
        .iter()
        .map(|r| {
            vec![
                rec_short_id(&r.id),
                humanize_relative(r.ts),
                r.method.clone(),
                r.path.clone(),
                r.status.to_string(),
                format!("{}ms", r.duration_ms),
            ]
        })
        .collect();
    print_table(
        &["ID", "TS", "METHOD", "PATH", "STATUS", "DURATION"],
        &table_rows,
    );
    Ok(())
}

async fn cmd_record_show(client: &Client, id: &str, fmt: OutputFormat) -> Result<()> {
    let rec: Recording = client.get(&format!("/recordings/{id}")).await?;
    if fmt != OutputFormat::Table {
        return emit(&rec, fmt);
    }

    println!(
        "{} {} ({})",
        bold(&rec.request.method),
        rec.request.path,
        dim(&rec.id)
    );
    println!(
        "  {} {} → status {} ({}ms)",
        dim("upstream"),
        if rec.upstream_url.is_empty() {
            "<unknown>"
        } else {
            rec.upstream_url.as_str()
        },
        rec.response.status,
        rec.duration_ms
    );
    if let Some(c) = rec.consumer.as_deref() {
        println!("  {} {}", dim("consumer"), c);
    }
    if let Some(s) = rec.service_id {
        println!("  {} {}", dim("service"), s);
    }
    if let Some(r) = rec.route_id {
        println!("  {} {}", dim("route"), r);
    }

    println!("\n{}", bold("Request headers"));
    for (k, v) in &rec.request.headers {
        println!("  {}: {}", dim(k), v);
    }
    if !rec.request.body_b64.is_empty() {
        println!("\n{}", bold("Request body"));
        print_decoded_body(&rec.request.body_b64);
    }

    println!("\n{}", bold("Response headers"));
    for (k, v) in &rec.response.headers {
        println!("  {}: {}", dim(k), v);
    }
    if !rec.response.body_b64.is_empty() {
        println!("\n{}", bold("Response body"));
        print_decoded_body(&rec.response.body_b64);
    }
    Ok(())
}

fn print_decoded_body(b64: &str) {
    use base64::{engine::general_purpose::STANDARD, Engine as _};
    match STANDARD.decode(b64) {
        Ok(bytes) => match std::str::from_utf8(&bytes) {
            Ok(s) => match serde_json::from_str::<Json>(s) {
                Ok(j) => {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&j).unwrap_or_else(|_| s.to_string())
                    );
                }
                Err(_) => {
                    println!("{s}");
                }
            },
            Err(_) => {
                println!("{}", dim(&format!("(binary, {} bytes)", bytes.len())));
            }
        },
        Err(e) => {
            println!("{}", dim(&format!("(undecodable: {e})")));
        }
    }
}

async fn cmd_record_replay(
    client: &Client,
    id: &str,
    against: Option<String>,
    fmt: OutputFormat,
) -> Result<()> {
    let body = serde_json::json!({ "target_url": against });
    let result: ReplayResult = client
        .post(&format!("/recordings/{id}/replay"), &body)
        .await?;
    if fmt != OutputFormat::Table {
        return emit(&result, fmt);
    }
    render_replay_diff(&result);
    Ok(())
}

fn render_replay_diff(r: &ReplayResult) {
    println!(
        "{} replay {} against {} ({}ms)",
        bold("→"),
        rec_short_id(&r.original.id),
        r.target_url,
        r.elapsed_ms
    );
    match r.diff.status_changed {
        Some((before, after)) => {
            let line = format!("status: {before} → {after}");
            println!("  {}", red(&line));
        }
        None => {
            println!("  {} status: {}", green("✓"), r.new_response.status);
        }
    }
    for (k, v) in &r.diff.headers_added {
        println!("  {} {}: {}", green("+"), k, truncate_for_diff(v));
    }
    for (k, v) in &r.diff.headers_removed {
        println!("  {} {}: {}", red("-"), k, truncate_for_diff(v));
    }
    for (k, before, after) in &r.diff.headers_changed {
        println!(
            "  {} {}: {} → {}",
            yellow("~"),
            k,
            truncate_for_diff(before),
            truncate_for_diff(after)
        );
    }
    match &r.diff.body_diff {
        BodyDiff::Identical => {
            println!("  {} body identical", green("✓"));
        }
        BodyDiff::JsonDiff {
            added,
            removed,
            changed,
        } => {
            println!("  {}", bold("body (JSON):"));
            if let Some(obj) = added.as_object() {
                for (k, v) in obj {
                    println!(
                        "    {} {}: {}",
                        green("+"),
                        k,
                        truncate_for_diff(&v.to_string())
                    );
                }
            }
            if let Some(obj) = removed.as_object() {
                for (k, v) in obj {
                    println!(
                        "    {} {}: {}",
                        red("-"),
                        k,
                        truncate_for_diff(&v.to_string())
                    );
                }
            }
            if let Some(obj) = changed.as_object() {
                for (k, v) in obj {
                    let before = v.get("before").map(|x| x.to_string()).unwrap_or_default();
                    let after = v.get("after").map(|x| x.to_string()).unwrap_or_default();
                    println!(
                        "    {} {}: {} → {}",
                        yellow("~"),
                        k,
                        truncate_for_diff(&before),
                        truncate_for_diff(&after)
                    );
                }
            }
        }
        BodyDiff::TextDiff { lines_changed } => {
            println!("  {} body diff: {lines_changed} lines changed", yellow("~"));
        }
        BodyDiff::BinaryDiff {
            before_bytes,
            after_bytes,
        } => {
            println!(
                "  {} binary body: {before_bytes} → {after_bytes} bytes",
                yellow("~")
            );
        }
    }
}

fn truncate_for_diff(s: &str) -> String {
    if s.len() > 80 {
        format!("{}…", &s[..80])
    } else {
        s.to_string()
    }
}

async fn cmd_record_delete(client: &Client, id: &str) -> Result<()> {
    client.delete(&format!("/recordings/{id}")).await?;
    println!("{} deleted recording {}", green("✓"), rec_short_id(id));
    Ok(())
}

/// Tiny URL-encoder for query-string values. Avoids pulling in `url` just
/// for this; we only need to escape a handful of bytes.
fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

async fn cmd_consumers_list(client: &Client, fmt: OutputFormat) -> Result<()> {
    let consumers: Vec<Consumer> = client.get("/consumers").await?;
    if fmt != OutputFormat::Table {
        return emit(&consumers, fmt);
    }
    let rows: Vec<Vec<String>> = consumers
        .iter()
        .map(|c| {
            vec![
                fmt_id(&c.id),
                c.username.clone(),
                c.custom_id.clone().unwrap_or_default(),
                c.api_key
                    .clone()
                    .map(|k| {
                        let n = k.len().min(8);
                        format!("{}…", &k[..n])
                    })
                    .unwrap_or_default(),
            ]
        })
        .collect();
    print_table(&["ID", "USERNAME", "CUSTOM_ID", "API_KEY"], &rows);
    Ok(())
}

async fn cmd_consumers_create(
    client: &Client,
    username: String,
    custom_id: Option<String>,
    fmt: OutputFormat,
) -> Result<()> {
    let body = serde_json::json!({ "username": username, "custom_id": custom_id });
    let consumer: Consumer = client.post("/consumers", &body).await?;
    if fmt == OutputFormat::Table {
        println!(
            "{} created consumer {} ({})",
            green("✓"),
            consumer.username,
            fmt_id(&consumer.id)
        );
    } else {
        emit(&consumer, fmt)?;
    }
    Ok(())
}

async fn cmd_consumers_delete(client: &Client, ident: &str) -> Result<()> {
    let consumer = resolve_consumer(client, ident).await?;
    client
        .delete(&format!("/consumers/{}", consumer.id))
        .await?;
    println!(
        "{} deleted consumer {} ({})",
        green("✓"),
        consumer.username,
        fmt_id(&consumer.id)
    );
    Ok(())
}

async fn cmd_targets_list(
    client: &Client,
    service: Option<String>,
    fmt: OutputFormat,
) -> Result<()> {
    let targets: Vec<Target> = match &service {
        Some(s) => {
            let svc = resolve_service(client, s).await?;
            client.get(&format!("/services/{}/targets", svc.id)).await?
        }
        None => client.get("/targets").await?,
    };
    if fmt != OutputFormat::Table {
        return emit(&targets, fmt);
    }
    let services: Vec<Service> = client.get("/services").await.unwrap_or_default();
    let by_id: HashMap<Uuid, String> = services.iter().map(|s| (s.id, s.name.clone())).collect();
    let rows: Vec<Vec<String>> = targets
        .iter()
        .map(|t| {
            vec![
                fmt_id(&t.id),
                by_id
                    .get(&t.service_id)
                    .cloned()
                    .unwrap_or_else(|| fmt_id(&t.service_id)),
                t.url.clone(),
                t.weight.to_string(),
                if t.enabled { "yes".into() } else { "no".into() },
            ]
        })
        .collect();
    print_table(&["ID", "SERVICE", "URL", "WEIGHT", "ENABLED"], &rows);
    Ok(())
}

async fn cmd_targets_add(
    client: &Client,
    service: String,
    url: String,
    weight: u32,
    fmt: OutputFormat,
) -> Result<()> {
    let svc = resolve_service(client, &service).await?;
    let body = serde_json::json!({ "url": url, "weight": weight, "enabled": true });
    let target: Target = client
        .post(&format!("/services/{}/targets", svc.id), &body)
        .await?;
    if fmt == OutputFormat::Table {
        println!("{} added target {} → {}", green("✓"), target.url, svc.name);
    } else {
        emit(&target, fmt)?;
    }
    Ok(())
}

async fn cmd_targets_delete(client: &Client, id: &str) -> Result<()> {
    let uuid = Uuid::parse_str(id).context("target id must be a UUID")?;
    client.delete(&format!("/targets/{uuid}")).await?;
    println!("{} deleted target {}", green("✓"), fmt_id(&uuid));
    Ok(())
}

async fn cmd_health(client: &Client, fmt: OutputFormat) -> Result<()> {
    let health: Vec<ServiceHealth> = client.get("/health").await?;
    if fmt != OutputFormat::Table {
        return emit(&health, fmt);
    }
    let services: Vec<Service> = client.get("/services").await.unwrap_or_default();
    let by_id: HashMap<Uuid, String> = services.iter().map(|s| (s.id, s.name.clone())).collect();
    let rows: Vec<Vec<String>> = health
        .iter()
        .map(|h| {
            vec![
                by_id
                    .get(&h.service_id)
                    .cloned()
                    .unwrap_or_else(|| fmt_id(&h.service_id)),
                h.status.clone(),
                h.circuit.clone(),
                h.consecutive_failures.to_string(),
                h.last_check_at
                    .map(|t| t.to_rfc3339())
                    .unwrap_or_else(|| "—".into()),
                h.last_error
                    .clone()
                    .unwrap_or_else(|| "—".into())
                    .chars()
                    .take(60)
                    .collect(),
            ]
        })
        .collect();
    print_table(
        &[
            "SERVICE",
            "STATUS",
            "CIRCUIT",
            "FAILS",
            "LAST_CHECK",
            "LAST_ERROR",
        ],
        &rows,
    );
    Ok(())
}

async fn cmd_reload(client: &Client) -> Result<()> {
    let v = client.post_empty("/reload").await?;
    println!("{} reload triggered", green("✓"));
    if !v.is_null() {
        println!("{}", dim(&serde_json::to_string_pretty(&v)?));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Dump
// ---------------------------------------------------------------------------

async fn cmd_dump(client: &Client) -> Result<()> {
    let services: Vec<Service> = client.get("/services").await?;
    let routes: Vec<Route> = client.get("/routes").await?;
    let plugins: Vec<PluginInstance> = client.get("/plugins").await?;
    let targets: Vec<Target> = client.get("/targets").await.unwrap_or_default();

    let svc_by_id: HashMap<Uuid, &Service> = services.iter().map(|s| (s.id, s)).collect();
    let route_by_id: HashMap<Uuid, &Route> = routes.iter().map(|r| (r.id, r)).collect();

    let mut targets_by_svc: HashMap<Uuid, Vec<DeclTarget>> = HashMap::new();
    for t in &targets {
        targets_by_svc
            .entry(t.service_id)
            .or_default()
            .push(DeclTarget {
                url: t.url.clone(),
                weight: t.weight,
            });
    }

    let decl_services: Vec<DeclService> = services
        .iter()
        .map(|s| DeclService {
            name: s.name.clone(),
            url: s.url.clone(),
            retries: s.retries,
            connect_timeout_ms: s.connect_timeout_ms,
            lb_strategy: s.lb_strategy.clone().unwrap_or_else(default_lb),
            targets: targets_by_svc.remove(&s.id).unwrap_or_default(),
        })
        .collect();

    let decl_routes: Vec<DeclRoute> = routes
        .iter()
        .map(|r| DeclRoute {
            service: svc_by_id
                .get(&r.service_id)
                .map(|s| s.name.clone())
                .unwrap_or_else(|| r.service_id.to_string()),
            paths: r.paths.clone(),
            methods: r.methods.clone(),
            strip_path: r.strip_path,
        })
        .collect();

    let decl_plugins: Vec<DeclPlugin> = plugins
        .iter()
        .map(|p| {
            let (scope, service, route) = match (p.service_id, p.route_id) {
                (Some(sid), _) => (
                    "service".into(),
                    svc_by_id.get(&sid).map(|s| s.name.clone()),
                    None,
                ),
                (_, Some(rid)) => (
                    "route".into(),
                    None,
                    route_by_id.get(&rid).map(|r| r.paths.join(";")),
                ),
                _ => ("global".into(), None, None),
            };
            DeclPlugin {
                name: p.name.clone(),
                scope,
                service,
                route,
                config: p.config.clone(),
            }
        })
        .collect();

    let state = DeclarativeState {
        services: decl_services,
        routes: decl_routes,
        plugins: decl_plugins,
    };
    print!("{}", serde_yaml::to_string(&state)?);
    Ok(())
}

// ---------------------------------------------------------------------------
// Apply: diff plan + ordered execution.
// ---------------------------------------------------------------------------

// A simple plan entry, rendered like `+ service api-backend` etc.
#[derive(Debug, Clone)]
struct PlanEntry {
    sigil: char,        // '+', '~', '-'
    kind: &'static str, // "service", "route", "plugin", "target"
    label: String,
    detail: Option<String>,
    /// Execution stage: 0=svc-upsert, 1=tgt, 2=route-upsert, 3=plugin-upsert,
    /// 4=plugin-delete, 5=route-delete, 6=svc-delete
    stage: u8,
    action: PlanAction,
}

#[derive(Debug, Clone)]
enum PlanAction {
    CreateService(DeclService),
    UpdateService {
        id: Uuid,
        ds: DeclService,
    },
    DeleteService {
        id: Uuid,
    },
    AddTarget {
        service_id: Uuid,
        url: String,
        weight: u32,
    },
    UpdateTarget {
        id: Uuid,
        weight: u32,
    },
    DeleteTarget {
        id: Uuid,
    },
    CreateRoute {
        service_name: String,
        paths: Vec<String>,
        methods: Vec<String>,
        strip_path: bool,
    },
    DeleteRoute {
        id: Uuid,
    },
    CreatePlugin {
        name: String,
        service: Option<String>,
        route_paths: Option<Vec<String>>,
        config: Json,
    },
    DeletePlugin {
        id: Uuid,
    },
}

fn render_plan(entries: &[PlanEntry]) {
    for e in entries {
        let sigil_str = e.sigil.to_string();
        let colored = match e.sigil {
            '+' => green(&sigil_str),
            '~' => yellow(&sigil_str),
            '-' => red(&sigil_str),
            _ => sigil_str.clone(),
        };
        let detail = match &e.detail {
            Some(d) => format!(" {}", dim(&format!("({d})"))),
            None => String::new(),
        };
        println!("{colored} {} {}{}", e.kind, e.label, detail);
    }
}

fn confirm(prompt: &str) -> Result<bool> {
    if !io::stdin().is_terminal() {
        // No TTY → safest to bail out and require --auto-approve
        bail!("non-interactive shell; pass --auto-approve to confirm");
    }
    eprint!("{prompt}");
    let _ = io::stderr().flush();
    let mut buf = String::new();
    io::stdin().read_line(&mut buf)?;
    let s = buf.trim().to_ascii_lowercase();
    Ok(s == "y" || s == "yes")
}

// ---------------------------------------------------------------------------
// `lynxx import` — OpenAPI 3.x → DeclarativeState → reuse cmd_apply pipeline.
//
// Translation contract (deliberately simple, deliberately deterministic so
// re-imports produce zero churn):
//
// - One Lynxx service per unique upstream URL. Multiple operations against
//   the same upstream collapse onto that one service.
// - One Lynxx route per (operation, path-template). Path params `{x}` are
//   converted to `*` since Lynxx route matching is wildcard-based.
// - Service name: `<prefix>-<slug>` where slug is the spec's `operationId`
//   if present, else `<method>-<path>` slugified. Prefix optional.
// - request-validator plugin attached when the operation has a requestBody
//   with an `application/json` schema. (--validators=false to skip.)
// - openapi-mock plugin attached per route only when --mock is set; its
//   spec_path is the importer's source file.
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
async fn cmd_import(
    client: &Client,
    spec_src: String,
    service_prefix: Option<String>,
    upstream_override: Option<String>,
    strip_base_path: bool,
    validators: bool,
    mock: bool,
    dry_run: bool,
    auto_approve: bool,
) -> Result<()> {
    let raw = if spec_src.starts_with("http://") || spec_src.starts_with("https://") {
        let resp = reqwest::Client::new()
            .get(&spec_src)
            .send()
            .await
            .with_context(|| format!("GET {spec_src}"))?;
        if !resp.status().is_success() {
            bail!("GET {spec_src} → HTTP {}", resp.status());
        }
        resp.text().await.context("reading spec body")?
    } else {
        fs::read_to_string(&spec_src).with_context(|| format!("reading {spec_src}"))?
    };

    let spec: openapiv3::OpenAPI = match serde_json::from_str::<openapiv3::OpenAPI>(&raw) {
        Ok(s) => s,
        Err(_) => serde_yaml::from_str::<openapiv3::OpenAPI>(&raw)
            .context("parsing OpenAPI spec (tried JSON then YAML)")?,
    };

    let state = openapi_to_state(
        &spec,
        &spec_src,
        service_prefix.as_deref(),
        upstream_override.as_deref(),
        strip_base_path,
        validators,
        mock,
    )?;

    if state.services.is_empty() {
        eprintln!(
            "{} spec produced no services — check `servers[0].url` or pass --upstream",
            yellow("!")
        );
        return Ok(());
    }

    apply_state(client, state, dry_run, auto_approve).await
}

/// Convert a parsed OpenAPI doc into the gateway's `DeclarativeState`.
///
/// We surface bad input as `anyhow` errors with enough context for an
/// operator to fix the spec; we do **not** silently drop operations.
#[allow(clippy::too_many_arguments)]
fn openapi_to_state(
    spec: &openapiv3::OpenAPI,
    spec_source: &str,
    service_prefix: Option<&str>,
    upstream_override: Option<&str>,
    strip_base_path: bool,
    install_validators: bool,
    install_mock: bool,
) -> Result<DeclarativeState> {
    use openapiv3::ReferenceOr;

    let upstream_default = upstream_override
        .map(|s| s.to_string())
        .or_else(|| spec.servers.first().map(|s| s.url.clone()))
        .ok_or_else(|| {
            anyhow!("no upstream URL: pass --upstream or add `servers[0].url` to the spec")
        })?;

    // One service per unique upstream URL. We keep insertion order stable
    // so re-imports of the same spec produce the same service names.
    let mut services_by_url: indexed::IndexMap<String, String> = indexed::IndexMap::new();
    let mut routes: Vec<DeclRoute> = Vec::new();
    let mut plugins: Vec<DeclPlugin> = Vec::new();

    for (path_tmpl, item_ref) in &spec.paths.paths {
        let item = match item_ref {
            ReferenceOr::Item(it) => it,
            ReferenceOr::Reference { .. } => continue,
        };

        let methods: [(&str, &Option<openapiv3::Operation>); 8] = [
            ("GET", &item.get),
            ("POST", &item.post),
            ("PUT", &item.put),
            ("DELETE", &item.delete),
            ("PATCH", &item.patch),
            ("HEAD", &item.head),
            ("OPTIONS", &item.options),
            ("TRACE", &item.trace),
        ];

        for (method, op_opt) in methods {
            let op = match op_opt {
                Some(o) => o,
                None => continue,
            };

            // Service name slug: prefix + (operationId | method-path).
            let slug = match &op.operation_id {
                Some(id) => slugify(id),
                None => slugify(&format!("{}-{}", method.to_lowercase(), path_tmpl)),
            };
            let service_name = match service_prefix {
                Some(p) => format!("{}-{}", slugify(p), slug),
                None => slug.clone(),
            };

            // Route path: `{name}` → `*`.
            let route_path = openapi_path_to_lynxx(path_tmpl);

            // Reuse an existing service for this upstream URL; otherwise create one
            // and register the slug we derived. Per the design, service names
            // collapse on shared upstream so we keep the *first* slug we saw.
            let svc_name_final = services_by_url
                .entry(upstream_default.clone())
                .or_insert_with(|| service_name.clone())
                .clone();

            routes.push(DeclRoute {
                service: svc_name_final.clone(),
                paths: vec![route_path.clone()],
                methods: vec![method.to_string()],
                strip_path: strip_base_path,
            });

            // request-validator plugin.
            if install_validators {
                if let Some(schema) = first_request_body_schema(op) {
                    plugins.push(DeclPlugin {
                        name: "request-validator".into(),
                        scope: "route".into(),
                        service: None,
                        route: Some(route_path.clone()),
                        config: serde_json::json!({ "body_schema": schema }),
                    });
                }
            }

            // openapi-mock plugin.
            if install_mock {
                plugins.push(DeclPlugin {
                    name: "openapi-mock".into(),
                    scope: "route".into(),
                    service: None,
                    route: Some(route_path.clone()),
                    config: serde_json::json!({
                        "spec_path": spec_source,
                        "default_status": 200,
                        "delay_ms": 0,
                        "passthrough_unmatched": false,
                    }),
                });
            }
        }
    }

    let services: Vec<DeclService> = services_by_url
        .iter()
        .map(|(url, name)| DeclService {
            name: name.clone(),
            url: url.clone(),
            retries: default_retries(),
            connect_timeout_ms: default_timeout(),
            lb_strategy: default_lb(),
            targets: Vec::new(),
        })
        .collect();

    Ok(DeclarativeState {
        services,
        routes,
        plugins,
    })
}

/// `/users/{id}/posts/{pid}` → `/users/*/posts/*`
fn openapi_path_to_lynxx(p: &str) -> String {
    let mut out = String::with_capacity(p.len());
    let mut in_param = false;
    for ch in p.chars() {
        match ch {
            '{' => {
                in_param = true;
                out.push('*');
            }
            '}' => in_param = false,
            _ if in_param => {}
            _ => out.push(ch),
        }
    }
    out
}

/// Slugify a name into something that's safe as a Lynxx service name. The
/// slugification is deterministic so re-imports of the same spec produce the
/// same names — that's the entire idempotency story.
fn slugify(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_dash = false;
    for ch in s.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            prev_dash = false;
        } else if !prev_dash && !out.is_empty() {
            out.push('-');
            prev_dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        out.push_str("op");
    }
    out
}

/// Pull the request-body schema for `application/json` (or the first content
/// type if there's no JSON one) and return it as raw JSON. Returns None if
/// the operation has no requestBody or its schema is a `$ref` we can't
/// dereference.
fn first_request_body_schema(op: &openapiv3::Operation) -> Option<serde_json::Value> {
    use openapiv3::ReferenceOr;
    let rb_ref = op.request_body.as_ref()?;
    let rb = match rb_ref {
        ReferenceOr::Item(b) => b,
        ReferenceOr::Reference { .. } => return None,
    };
    let media = rb
        .content
        .get("application/json")
        .or_else(|| rb.content.iter().next().map(|(_, v)| v))?;
    let schema_ref = media.schema.as_ref()?;
    serde_json::to_value(schema_ref).ok()
}

/// Tiny insertion-order map used by `openapi_to_state`. We don't take a
/// dependency on `indexmap` for this one site.
mod indexed {
    pub struct IndexMap<K, V> {
        keys: Vec<K>,
        values: Vec<V>,
    }
    impl<K: PartialEq + Clone, V: Clone> IndexMap<K, V> {
        pub fn new() -> Self {
            Self {
                keys: Vec::new(),
                values: Vec::new(),
            }
        }
        pub fn entry(&mut self, k: K) -> Entry<'_, K, V> {
            let idx = self.keys.iter().position(|x| x == &k);
            Entry {
                map: self,
                key: k,
                idx,
            }
        }
        pub fn iter(&self) -> impl Iterator<Item = (&K, &V)> {
            self.keys.iter().zip(self.values.iter())
        }
    }
    pub struct Entry<'a, K, V> {
        map: &'a mut IndexMap<K, V>,
        key: K,
        idx: Option<usize>,
    }
    impl<'a, K: Clone, V: Clone> Entry<'a, K, V> {
        pub fn or_insert_with<F: FnOnce() -> V>(self, f: F) -> &'a mut V {
            match self.idx {
                Some(i) => &mut self.map.values[i],
                None => {
                    self.map.keys.push(self.key);
                    self.map.values.push(f());
                    self.map.values.last_mut().unwrap()
                }
            }
        }
    }
}

#[cfg(test)]
mod openapi_import_tests {
    use super::*;

    fn spec() -> openapiv3::OpenAPI {
        let raw = r#"
openapi: 3.0.3
info: { title: t, version: '1.0' }
servers:
  - url: https://api.example.com
paths:
  /users:
    get:
      operationId: listUsers
      responses: { '200': { description: ok } }
    post:
      operationId: createUser
      requestBody:
        required: true
        content:
          application/json:
            schema:
              type: object
              required: [email]
              properties: { email: { type: string } }
      responses: { '201': { description: created } }
  /users/{id}:
    get:
      operationId: getUser
      responses: { '200': { description: ok } }
"#;
        serde_yaml::from_str(raw).unwrap()
    }

    #[test]
    fn produces_one_service_per_upstream() {
        let s = openapi_to_state(&spec(), "spec.yaml", None, None, false, true, false).unwrap();
        assert_eq!(s.services.len(), 1);
        assert_eq!(s.services[0].url, "https://api.example.com");
    }

    #[test]
    fn produces_one_route_per_operation() {
        let s = openapi_to_state(&spec(), "spec.yaml", None, None, false, true, false).unwrap();
        assert_eq!(
            s.routes.len(),
            3,
            "GET /users, POST /users, GET /users/{{id}}"
        );
        assert!(s
            .routes
            .iter()
            .any(|r| r.paths == vec!["/users/*"] && r.methods == vec!["GET"]));
    }

    #[test]
    fn installs_validator_only_for_request_body_routes() {
        let s = openapi_to_state(&spec(), "spec.yaml", None, None, false, true, false).unwrap();
        let validator_count = s
            .plugins
            .iter()
            .filter(|p| p.name == "request-validator")
            .count();
        assert_eq!(validator_count, 1);
    }

    #[test]
    fn idempotent_reimport_produces_same_state() {
        let a = openapi_to_state(
            &spec(),
            "spec.yaml",
            Some("billing"),
            None,
            false,
            true,
            false,
        )
        .unwrap();
        let b = openapi_to_state(
            &spec(),
            "spec.yaml",
            Some("billing"),
            None,
            false,
            true,
            false,
        )
        .unwrap();
        assert_eq!(
            serde_yaml::to_string(&a).unwrap(),
            serde_yaml::to_string(&b).unwrap()
        );
    }

    #[test]
    fn mock_flag_adds_mock_plugin_per_route() {
        let s = openapi_to_state(&spec(), "spec.yaml", None, None, false, false, true).unwrap();
        let mock_count = s
            .plugins
            .iter()
            .filter(|p| p.name == "openapi-mock")
            .count();
        assert_eq!(mock_count, 3);
    }

    #[test]
    fn upstream_override_wins() {
        let s = openapi_to_state(
            &spec(),
            "spec.yaml",
            None,
            Some("https://override"),
            false,
            false,
            false,
        )
        .unwrap();
        assert_eq!(s.services[0].url, "https://override");
    }

    #[test]
    fn path_param_to_wildcard() {
        assert_eq!(openapi_path_to_lynxx("/users/{id}"), "/users/*");
        assert_eq!(openapi_path_to_lynxx("/a/{x}/b/{y}"), "/a/*/b/*");
        assert_eq!(openapi_path_to_lynxx("/static"), "/static");
    }

    #[test]
    fn slugify_is_stable() {
        assert_eq!(slugify("listUsers"), "listusers");
        assert_eq!(slugify("get-user_v2"), "get-user-v2");
        assert_eq!(slugify("/users/{id}"), "users-id");
    }
}

async fn cmd_apply(
    client: &Client,
    file: PathBuf,
    dry_run: bool,
    auto_approve: bool,
) -> Result<()> {
    let raw = if file == PathBuf::from("-") {
        let mut buf = String::new();
        io::stdin().read_to_string(&mut buf)?;
        buf
    } else {
        fs::read_to_string(&file).with_context(|| format!("reading {}", file.display()))?
    };
    let desired: DeclarativeState = serde_yaml::from_str(&raw).context("parsing state YAML")?;
    apply_state(client, desired, dry_run, auto_approve).await
}

/// Same body as `cmd_apply` but starting from an in-memory `DeclarativeState`.
/// Used by `lynxx import`, which builds a state from an OpenAPI spec rather
/// than reading YAML from disk.
async fn apply_state(
    client: &Client,
    desired: DeclarativeState,
    dry_run: bool,
    auto_approve: bool,
) -> Result<()> {
    // Snapshot current state.
    let cur_services: Vec<Service> = client.get("/services").await?;
    let cur_routes: Vec<Route> = client.get("/routes").await?;
    let cur_plugins: Vec<PluginInstance> = client.get("/plugins").await?;
    let cur_targets: Vec<Target> = client.get("/targets").await.unwrap_or_default();

    let svc_by_name: HashMap<String, &Service> =
        cur_services.iter().map(|s| (s.name.clone(), s)).collect();
    let svc_name_by_id: HashMap<Uuid, String> = cur_services
        .iter()
        .map(|s| (s.id, s.name.clone()))
        .collect();
    let want_svc_names: std::collections::HashSet<&str> =
        desired.services.iter().map(|s| s.name.as_str()).collect();

    let mut plan: Vec<PlanEntry> = Vec::new();

    // ---- services upsert (stage 0)
    for ds in &desired.services {
        match svc_by_name.get(ds.name.as_str()) {
            Some(existing) => {
                let mut diffs: Vec<&'static str> = Vec::new();
                if existing.url != ds.url {
                    diffs.push("url");
                }
                if existing.retries != ds.retries {
                    diffs.push("retries");
                }
                if existing.connect_timeout_ms != ds.connect_timeout_ms {
                    diffs.push("timeout");
                }
                if existing.lb_strategy.as_deref().unwrap_or("round-robin") != ds.lb_strategy {
                    diffs.push("lb_strategy");
                }
                if !diffs.is_empty() {
                    plan.push(PlanEntry {
                        sigil: '~',
                        kind: "service",
                        label: ds.name.clone(),
                        detail: Some(format!("{} changed", diffs.join(", "))),
                        stage: 0,
                        action: PlanAction::UpdateService {
                            id: existing.id,
                            ds: ds.clone(),
                        },
                    });
                }
            }
            None => plan.push(PlanEntry {
                sigil: '+',
                kind: "service",
                label: ds.name.clone(),
                detail: Some(ds.url.clone()),
                stage: 0,
                action: PlanAction::CreateService(ds.clone()),
            }),
        }
    }

    // ---- targets diff (stage 1) — uses current snapshot for ids
    let mut targets_by_svc: HashMap<Uuid, Vec<&Target>> = HashMap::new();
    for t in &cur_targets {
        targets_by_svc.entry(t.service_id).or_default().push(t);
    }
    for ds in &desired.services {
        // Look up current id (if it exists pre-apply)
        let existing_svc_id = svc_by_name.get(ds.name.as_str()).map(|s| s.id);
        let have: BTreeMap<String, (Uuid, u32)> = existing_svc_id
            .and_then(|sid| targets_by_svc.get(&sid))
            .map(|v| {
                v.iter()
                    .map(|t| (t.url.clone(), (t.id, t.weight)))
                    .collect()
            })
            .unwrap_or_default();
        let want: BTreeMap<String, u32> = ds
            .targets
            .iter()
            .map(|t| (t.url.clone(), t.weight))
            .collect();

        for (url, weight) in &want {
            match have.get(url) {
                Some((_, w)) if w == weight => {}
                Some((tid, _)) => plan.push(PlanEntry {
                    sigil: '~',
                    kind: "target",
                    label: format!("{} ({})", url, ds.name),
                    detail: Some(format!("weight={weight}")),
                    stage: 1,
                    action: PlanAction::UpdateTarget {
                        id: *tid,
                        weight: *weight,
                    },
                }),
                None => {
                    // Need to defer service_id resolution until after stage 0.
                    // We mark the action with the pre-existing id if any; otherwise
                    // it'll be resolved at execution time via the post-stage-0 map.
                    let svc_id_marker = existing_svc_id.unwrap_or(Uuid::nil());
                    plan.push(PlanEntry {
                        sigil: '+',
                        kind: "target",
                        label: format!("{} ({})", url, ds.name),
                        detail: Some(format!("weight={weight}")),
                        stage: 1,
                        action: PlanAction::AddTarget {
                            service_id: svc_id_marker,
                            url: url.clone(),
                            weight: *weight,
                        },
                    });
                }
            }
        }
        for (url, (tid, _)) in &have {
            if !want.contains_key(url) {
                plan.push(PlanEntry {
                    sigil: '-',
                    kind: "target",
                    label: format!("{} ({})", url, ds.name),
                    detail: None,
                    stage: 5, // delete-stage; before route deletes
                    action: PlanAction::DeleteTarget { id: *tid },
                });
            }
        }
    }

    // ---- routes diff (stage 2 create / 6 delete)
    fn route_key(svc: &str, paths: &[String]) -> String {
        let mut p = paths.to_vec();
        p.sort();
        format!("{svc}|{}", p.join(","))
    }
    let cur_route_keys: HashMap<String, &Route> = cur_routes
        .iter()
        .map(|r| {
            let svc = svc_name_by_id
                .get(&r.service_id)
                .cloned()
                .unwrap_or_else(|| r.service_id.to_string());
            (route_key(&svc, &r.paths), r)
        })
        .collect();
    let want_route_keys: std::collections::HashSet<String> = desired
        .routes
        .iter()
        .map(|r| route_key(&r.service, &r.paths))
        .collect();

    for dr in &desired.routes {
        let key = route_key(&dr.service, &dr.paths);
        if cur_route_keys.contains_key(&key) {
            continue;
        }
        plan.push(PlanEntry {
            sigil: '+',
            kind: "route",
            label: dr.paths.join(","),
            detail: Some(format!("→ {}", dr.service)),
            stage: 2,
            action: PlanAction::CreateRoute {
                service_name: dr.service.clone(),
                paths: dr.paths.clone(),
                methods: dr.methods.clone(),
                strip_path: dr.strip_path,
            },
        });
    }
    for (key, route) in &cur_route_keys {
        if !want_route_keys.contains(key) {
            plan.push(PlanEntry {
                sigil: '-',
                kind: "route",
                label: route.paths.join(","),
                detail: None,
                stage: 6,
                action: PlanAction::DeleteRoute { id: route.id },
            });
        }
    }

    // ---- plugins diff (stage 3 create / 4 delete)
    fn plugin_key(name: &str, scope: &str, target: &str) -> String {
        format!("{name}|{scope}|{target}")
    }
    let cur_plugin_keys: HashMap<String, &PluginInstance> = cur_plugins
        .iter()
        .map(|p| {
            let (scope, target) = match (p.service_id, p.route_id) {
                (Some(s), _) => (
                    "service",
                    svc_name_by_id
                        .get(&s)
                        .cloned()
                        .unwrap_or_else(|| s.to_string()),
                ),
                (_, Some(r)) => {
                    let k = cur_routes
                        .iter()
                        .find(|rt| rt.id == r)
                        .map(|rt| {
                            let svc = svc_name_by_id
                                .get(&rt.service_id)
                                .cloned()
                                .unwrap_or_else(|| rt.service_id.to_string());
                            let mut ps = rt.paths.clone();
                            ps.sort();
                            format!("{}|{}", svc, ps.join(";"))
                        })
                        .unwrap_or_else(|| r.to_string());
                    ("route", k)
                }
                _ => ("global", String::new()),
            };
            (plugin_key(&p.name, scope, &target), p)
        })
        .collect();

    let mut want_plugin_keys: std::collections::HashSet<String> = Default::default();
    for dp in &desired.plugins {
        let (scope, target, route_paths_opt) = match dp.scope.as_str() {
            "service" => {
                let s = dp.service.as_deref().ok_or_else(|| {
                    anyhow!("plugin {} scope=service requires `service:`", dp.name)
                })?;
                ("service", s.to_string(), None)
            }
            "route" => {
                let r = dp.route.as_deref().ok_or_else(|| {
                    anyhow!(
                        "plugin {} scope=route requires `route:` (paths joined by ';')",
                        dp.name
                    )
                })?;
                let mut ps: Vec<String> = r.split(';').map(|s| s.to_string()).collect();
                ps.sort();
                let joined = ps.join(";");
                let svc = cur_routes
                    .iter()
                    .find(|rt| {
                        let mut got = rt.paths.clone();
                        got.sort();
                        got.join(";") == joined
                    })
                    .and_then(|rt| svc_name_by_id.get(&rt.service_id).cloned())
                    .unwrap_or_else(|| "?".into());
                ("route", format!("{svc}|{joined}"), Some(ps))
            }
            "global" => ("global", String::new(), None),
            other => bail!("unknown plugin scope '{other}'"),
        };
        let key = plugin_key(&dp.name, scope, &target);
        want_plugin_keys.insert(key.clone());

        if !cur_plugin_keys.contains_key(&key) {
            let scope_label = if target.is_empty() {
                scope.to_string()
            } else {
                format!("{scope} {target}")
            };
            plan.push(PlanEntry {
                sigil: '+',
                kind: "plugin",
                label: dp.name.clone(),
                detail: Some(scope_label),
                stage: 3,
                action: PlanAction::CreatePlugin {
                    name: dp.name.clone(),
                    service: if scope == "service" {
                        Some(target.clone())
                    } else {
                        None
                    },
                    route_paths: route_paths_opt,
                    config: dp.config.clone(),
                },
            });
        }
    }
    for (key, p) in &cur_plugin_keys {
        if !want_plugin_keys.contains(key) {
            plan.push(PlanEntry {
                sigil: '-',
                kind: "plugin",
                label: p.name.clone(),
                detail: Some(fmt_id(&p.id)),
                stage: 4,
                action: PlanAction::DeletePlugin { id: p.id },
            });
        }
    }

    // ---- service deletes (stage 7)
    for s in &cur_services {
        if !want_svc_names.contains(s.name.as_str()) {
            plan.push(PlanEntry {
                sigil: '-',
                kind: "service",
                label: s.name.clone(),
                detail: None,
                stage: 7,
                action: PlanAction::DeleteService { id: s.id },
            });
        }
    }

    // Sort by execution stage (rendering follows stage order — natural plan readout).
    plan.sort_by_key(|e| e.stage);

    if plan.is_empty() {
        println!("{} nothing to do", green("✓"));
        return Ok(());
    }

    render_plan(&plan);

    if dry_run {
        eprintln!(
            "{}",
            dim(&format!("(dry-run: {} change(s) not applied)", plan.len()))
        );
        return Ok(());
    }

    if !auto_approve {
        eprintln!();
        let ok = confirm(&format!("Apply {} change(s)? (y/N): ", plan.len()))?;
        if !ok {
            eprintln!("{}", dim("aborted."));
            return Ok(());
        }
    }

    // Re-snapshot service ids if we created any (stage 0). We do this lazily.
    let mut svc_id_by_name: HashMap<String, Uuid> = cur_services
        .iter()
        .map(|s| (s.name.clone(), s.id))
        .collect();
    let mut route_id_by_key: HashMap<String, Uuid> = cur_routes
        .iter()
        .map(|r| {
            let svc = svc_name_by_id
                .get(&r.service_id)
                .cloned()
                .unwrap_or_else(|| r.service_id.to_string());
            let mut ps = r.paths.clone();
            ps.sort();
            (format!("{}|{}", svc, ps.join(";")), r.id)
        })
        .collect();

    let mut applied = 0usize;
    for entry in &plan {
        let res = apply_one(
            client,
            &entry.action,
            &mut svc_id_by_name,
            &mut route_id_by_key,
        )
        .await;
        match res {
            Ok(()) => {
                applied += 1;
                let s = entry.sigil.to_string();
                let colored = match entry.sigil {
                    '+' => green(&s),
                    '~' => yellow(&s),
                    '-' => red(&s),
                    _ => s.clone(),
                };
                println!("  {} {} {}", colored, dim("applied"), entry.label);
            }
            Err(e) => {
                eprintln!(
                    "{} step failed at stage {}: {} {}",
                    red("✗"),
                    entry.stage,
                    entry.kind,
                    entry.label
                );
                eprintln!("  {} {:#}", dim("└─"), e);
                eprintln!(
                    "{}",
                    dim(&format!(
                        "(stopped after {applied}/{} ops; partial state was NOT rolled back)",
                        plan.len()
                    ))
                );
                return Err(e);
            }
        }
    }

    println!("{} applied {} change(s)", green("✓"), applied);
    Ok(())
}

async fn apply_one(
    client: &Client,
    action: &PlanAction,
    svc_id_by_name: &mut HashMap<String, Uuid>,
    route_id_by_key: &mut HashMap<String, Uuid>,
) -> Result<()> {
    match action {
        PlanAction::CreateService(ds) => {
            let body = serde_json::json!({
                "name": ds.name, "url": ds.url, "retries": ds.retries,
                "connect_timeout_ms": ds.connect_timeout_ms,
                "lb_strategy": ds.lb_strategy,
            });
            let svc: Service = client.post("/services", &body).await?;
            svc_id_by_name.insert(svc.name.clone(), svc.id);
            Ok(())
        }
        PlanAction::UpdateService { id, ds } => {
            let body = serde_json::json!({
                "name": ds.name, "url": ds.url, "retries": ds.retries,
                "connect_timeout_ms": ds.connect_timeout_ms,
                "lb_strategy": ds.lb_strategy,
            });
            let url = format!("{}/services/{}", client.base, id);
            let resp = client.http.put(&url).json(&body).send().await?;
            let status = resp.status();
            if !status.is_success() {
                let text = resp.text().await.unwrap_or_default();
                return Err(ApiError {
                    method: "PUT",
                    url,
                    status,
                    message: parse_error_envelope(&text),
                    body: text,
                }
                .into());
            }
            Ok(())
        }
        PlanAction::DeleteService { id } => client.delete(&format!("/services/{}", id)).await,
        PlanAction::AddTarget {
            service_id,
            url,
            weight,
        } => {
            let sid = if service_id.is_nil() {
                bail!("internal: target add missing service id")
            } else {
                *service_id
            };
            let body = serde_json::json!({ "url": url, "weight": weight, "enabled": true });
            let _: Target = client
                .post(&format!("/services/{}/targets", sid), &body)
                .await?;
            Ok(())
        }
        PlanAction::UpdateTarget { id, weight } => {
            let body = serde_json::json!({ "weight": weight });
            let url = format!("{}/targets/{}", client.base, id);
            let resp = client.http.put(&url).json(&body).send().await?;
            let status = resp.status();
            if !status.is_success() {
                let text = resp.text().await.unwrap_or_default();
                return Err(ApiError {
                    method: "PUT",
                    url,
                    status,
                    message: parse_error_envelope(&text),
                    body: text,
                }
                .into());
            }
            Ok(())
        }
        PlanAction::DeleteTarget { id } => client.delete(&format!("/targets/{}", id)).await,
        PlanAction::CreateRoute {
            service_name,
            paths,
            methods,
            strip_path,
        } => {
            let svc_id = svc_id_by_name
                .get(service_name)
                .copied()
                .ok_or_else(|| anyhow!("route references unknown service '{service_name}'"))?;
            let body = serde_json::json!({
                "paths": paths, "methods": methods,
                "service_id": svc_id, "strip_path": strip_path,
            });
            let route: Route = client.post("/routes", &body).await?;
            let mut ps = route.paths.clone();
            ps.sort();
            route_id_by_key.insert(format!("{}|{}", service_name, ps.join(";")), route.id);
            Ok(())
        }
        PlanAction::DeleteRoute { id } => client.delete(&format!("/routes/{}", id)).await,
        PlanAction::CreatePlugin {
            name,
            service,
            route_paths,
            config,
        } => {
            let service_id =
                match service {
                    Some(s) => Some(svc_id_by_name.get(s).copied().ok_or_else(|| {
                        anyhow!("plugin {name} references unknown service '{s}'")
                    })?),
                    None => None,
                };
            let route_id = match route_paths {
                Some(ps) => {
                    let mut ps2 = ps.clone();
                    ps2.sort();
                    let joined = ps2.join(";");
                    // Find any route with matching paths set (across all services).
                    route_id_by_key.iter().find_map(|(k, v)| {
                        k.split_once('|').filter(|(_, p)| *p == joined).map(|_| *v)
                    })
                }
                None => None,
            };
            let body = serde_json::json!({
                "name": name, "service_id": service_id, "route_id": route_id,
                "enabled": true, "config": config,
            });
            let _: PluginInstance = client.post("/plugins", &body).await?;
            Ok(())
        }
        PlanAction::DeletePlugin { id } => client.delete(&format!("/plugins/{}", id)).await,
    }
}

// ---------------------------------------------------------------------------
// Validate
// ---------------------------------------------------------------------------

async fn cmd_validate(client: &Client, file: PathBuf) -> Result<()> {
    let raw = if file == PathBuf::from("-") {
        let mut buf = String::new();
        io::stdin().read_to_string(&mut buf)?;
        buf
    } else {
        fs::read_to_string(&file).with_context(|| format!("reading {}", file.display()))?
    };
    let state: DeclarativeState = serde_yaml::from_str(&raw).context("parsing state YAML")?;

    let mut errors: Vec<String> = Vec::new();

    // services[*].url
    for (i, s) in state.services.iter().enumerate() {
        match url::Url::parse(&s.url) {
            Ok(u) => {
                if u.scheme().is_empty() {
                    errors.push(format!("services[{i}].url: invalid URL: missing scheme"));
                }
            }
            Err(e) => errors.push(format!("services[{i}].url ({}): invalid URL: {}", s.url, e)),
        }
        for (j, t) in s.targets.iter().enumerate() {
            if let Err(e) = url::Url::parse(&t.url) {
                errors.push(format!(
                    "services[{i}].targets[{j}].url ({}): invalid URL: {}",
                    t.url, e
                ));
            }
        }
        if s.name.is_empty() {
            errors.push(format!("services[{i}].name: must not be empty"));
        }
    }

    // routes[*].service exists in desired services or current gateway
    let want_svc: std::collections::HashSet<&str> =
        state.services.iter().map(|s| s.name.as_str()).collect();
    let cur_services: Vec<Service> = client.get("/services").await.unwrap_or_default();
    let cur_svc_names: std::collections::HashSet<String> =
        cur_services.iter().map(|s| s.name.clone()).collect();

    for (i, r) in state.routes.iter().enumerate() {
        if !want_svc.contains(r.service.as_str()) && !cur_svc_names.contains(&r.service) {
            let suggest = fuzzy_suggest(
                &r.service,
                want_svc
                    .iter()
                    .copied()
                    .chain(cur_svc_names.iter().map(|s| s.as_str())),
            );
            let hint = suggest
                .map(|s| format!(". Did you mean '{s}'?"))
                .unwrap_or_default();
            errors.push(format!(
                "routes[{i}].service ('{}'): no such service in state or gateway{hint}",
                r.service
            ));
        }
        if r.paths.is_empty() {
            errors.push(format!("routes[{i}].paths: must not be empty"));
        }
        for (j, p) in r.paths.iter().enumerate() {
            if !p.starts_with('/') {
                errors.push(format!(
                    "routes[{i}].paths[{j}]: path must start with '/' (got '{p}')"
                ));
            }
        }
    }

    // plugins[*].name in catalog; config must be a JSON object (not scalar)
    for (i, p) in state.plugins.iter().enumerate() {
        if !PLUGIN_CATALOG.contains(&p.name.as_str()) {
            let suggest = fuzzy_suggest(&p.name, PLUGIN_CATALOG.iter().copied());
            let hint = suggest
                .map(|s| format!(". Did you mean '{s}'?"))
                .unwrap_or_default();
            errors.push(format!(
                "plugins[{i}].name ('{}'): unknown plugin{hint}",
                p.name
            ));
        }
        match &p.config {
            Json::Object(_) | Json::Null => {}
            _ => errors.push(format!(
                "plugins[{i}].config: must be a JSON object (got {})",
                match &p.config {
                    Json::String(_) => "string",
                    Json::Number(_) => "number",
                    Json::Bool(_) => "bool",
                    Json::Array(_) => "array",
                    _ => "scalar",
                }
            )),
        }
        match p.scope.as_str() {
            "global" => {}
            "service" => {
                if p.service.is_none() {
                    errors.push(format!("plugins[{i}]: scope=service requires `service:`"));
                } else if let Some(sn) = &p.service {
                    if !want_svc.contains(sn.as_str()) && !cur_svc_names.contains(sn) {
                        errors.push(format!(
                            "plugins[{i}].service ('{sn}'): no such service in state or gateway"
                        ));
                    }
                }
            }
            "route" => {
                if p.route.is_none() {
                    errors.push(format!(
                        "plugins[{i}]: scope=route requires `route:` (paths joined by ';')"
                    ));
                }
            }
            other => errors.push(format!("plugins[{i}].scope: unknown scope '{other}'")),
        }
    }

    if errors.is_empty() {
        let svc = state.services.len();
        let routes = state.routes.len();
        let plugins = state.plugins.len();
        println!(
            "{} valid: {svc} service(s), {routes} route(s), {plugins} plugin(s)",
            green("✓")
        );
        Ok(())
    } else {
        for e in &errors {
            println!("{} {}", red("✗"), e);
        }
        bail!("{} validation error(s)", errors.len());
    }
}

// ---------------------------------------------------------------------------
// Watch — periodic redraw of a list view.
// ---------------------------------------------------------------------------

fn install_ctrlc_handler() -> Arc<AtomicBool> {
    let stop = Arc::new(AtomicBool::new(false));
    let s = stop.clone();
    // Use tokio's signal facility to avoid pulling ctrlc crate.
    tokio::spawn(async move {
        if let Ok(()) = tokio::signal::ctrl_c().await {
            s.store(true, Ordering::SeqCst);
        }
    });
    stop
}

async fn cmd_watch(
    client: &Client,
    resource: WatchResource,
    interval: u64,
    fmt: OutputFormat,
) -> Result<()> {
    let stop = install_ctrlc_handler();
    let tty_out = io::stdout().is_terminal();

    if tty_out {
        // Hide cursor; we'll restore it on exit.
        print!("\x1b[?25l");
        let _ = io::stdout().flush();
    }

    let result: Result<()> = async {
        loop {
            if stop.load(Ordering::SeqCst) {
                break;
            }
            if tty_out {
                // Clear screen + home cursor.
                print!("\x1b[H\x1b[2J");
            }
            let now = Local::now().format("%Y-%m-%d %H:%M:%S");
            println!(
                "{} {} every {}s — Ctrl-C to exit",
                bold(&format!("lynxx watch {:?}", resource).to_lowercase()),
                dim(&format!("[{now}]")),
                interval
            );
            println!();

            let render = match resource {
                WatchResource::Services => cmd_services_list(client, fmt).await,
                WatchResource::Routes => cmd_routes_list(client, fmt).await,
                WatchResource::Plugins => cmd_plugins_list(client, fmt).await,
                WatchResource::Health => cmd_health(client, fmt).await,
            };
            if let Err(e) = render {
                println!("{} {:#}", red("✗"), e);
            }
            let _ = io::stdout().flush();

            // Sleep in small slices so Ctrl-C is responsive.
            let total = Duration::from_secs(interval.max(1));
            let start = Instant::now();
            while start.elapsed() < total {
                if stop.load(Ordering::SeqCst) {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
        Ok(())
    }
    .await;

    if tty_out {
        // Always restore cursor.
        print!("\x1b[?25h");
        let _ = io::stdout().flush();
    }
    result
}

// ---------------------------------------------------------------------------
// Tail — derive request deltas from /metrics each tick.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Eq, PartialEq, Hash)]
struct ReqKey {
    service: String,
    route: String,
    method: String,
    status: String, // "2xx", "404", etc.
}

/// Parse Prometheus text format. Returns counters keyed by metric name + sorted-label-tuple.
/// Captures per-line `metric{label="v",...} value` rows for `lynxx_requests_total` and
/// any `_seconds_sum` / `_seconds_count` pair if present.
fn parse_prom(text: &str) -> Vec<(String, Vec<(String, String)>, f64)> {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        // metric{labels} value [timestamp]
        let (head, value) = match line.rsplit_once(' ') {
            Some(v) => v,
            None => continue,
        };
        let value: f64 = match value.parse() {
            Ok(v) => v,
            Err(_) => continue,
        };
        let (name, labels) = if let Some(open) = head.find('{') {
            let close = match head.rfind('}') {
                Some(c) => c,
                None => continue,
            };
            let name = head[..open].to_string();
            let labels_str = &head[open + 1..close];
            let mut labels: Vec<(String, String)> = Vec::new();
            for kv in labels_str.split(',') {
                let kv = kv.trim();
                if kv.is_empty() {
                    continue;
                }
                let (k, v) = match kv.split_once('=') {
                    Some(s) => s,
                    None => continue,
                };
                let v = v.trim().trim_matches('"').to_string();
                labels.push((k.trim().to_string(), v));
            }
            labels.sort();
            (name, labels)
        } else {
            (head.to_string(), Vec::new())
        };
        out.push((name, labels, value));
    }
    out
}

fn status_bucket(s: &str) -> String {
    if let Ok(n) = s.parse::<u16>() {
        match n {
            100..=199 => "1xx".into(),
            200..=299 => "2xx".into(),
            300..=399 => "3xx".into(),
            400..=499 => "4xx".into(),
            500..=599 => "5xx".into(),
            _ => s.to_string(),
        }
    } else {
        s.to_string()
    }
}

async fn cmd_tail(client: &Client, interval: u64) -> Result<()> {
    let stop = install_ctrlc_handler();
    println!(
        "{} live request tail (interval {}s) — best-effort, derived from /metrics counters",
        dim("note:"),
        interval
    );
    println!("{}", dim("press Ctrl-C to exit"));

    let mut prev: HashMap<ReqKey, f64> = HashMap::new();
    let mut prev_latency: HashMap<ReqKey, (f64, f64)> = HashMap::new(); // (sum, count)
    let mut first = true;

    loop {
        if stop.load(Ordering::SeqCst) {
            break;
        }
        let text = match client.get_text("/metrics").await {
            Ok(t) => t,
            Err(e) => {
                eprintln!("{} {:#}", red("✗"), e);
                tokio::time::sleep(Duration::from_secs(interval.max(1))).await;
                continue;
            }
        };
        let metrics = parse_prom(&text);

        // Index counters and latency pairs.
        let mut current: HashMap<ReqKey, f64> = HashMap::new();
        let mut current_latency: HashMap<ReqKey, (f64, f64)> = HashMap::new();
        for (name, labels, value) in &metrics {
            let lkup = |k: &str| {
                labels
                    .iter()
                    .find(|(lk, _)| lk == k)
                    .map(|(_, v)| v.clone())
            };
            let key = || ReqKey {
                service: lkup("service").unwrap_or_default(),
                route: lkup("route").unwrap_or_else(|| lkup("path").unwrap_or_default()),
                method: lkup("method").unwrap_or_default(),
                status: status_bucket(&lkup("status").unwrap_or_default()),
            };
            match name.as_str() {
                "lynxx_requests_total" => {
                    let k = key();
                    *current.entry(k).or_insert(0.0) += value;
                }
                "lynxx_request_duration_seconds_sum" => {
                    let entry = current_latency.entry(key()).or_insert((0.0, 0.0));
                    entry.0 += value;
                }
                "lynxx_request_duration_seconds_count" => {
                    let entry = current_latency.entry(key()).or_insert((0.0, 0.0));
                    entry.1 += value;
                }
                _ => {}
            }
        }

        if !first {
            // Compute deltas; print one line per non-zero bucket.
            let mut deltas: Vec<(ReqKey, f64, Option<f64>)> = Vec::new();
            for (k, &v) in &current {
                let p = prev.get(k).copied().unwrap_or(0.0);
                let d = v - p;
                if d > 0.0 {
                    let lat = match (current_latency.get(k), prev_latency.get(k)) {
                        (Some(&(s2, c2)), Some(&(s1, c1))) if c2 > c1 => {
                            Some(((s2 - s1) / (c2 - c1)) * 1000.0)
                        }
                        (Some(&(s2, c2)), None) if c2 > 0.0 => Some((s2 / c2) * 1000.0),
                        _ => None,
                    };
                    deltas.push((k.clone(), d, lat));
                }
            }
            deltas.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
            let ts = Local::now().format("%H:%M:%S");
            for (k, count, lat) in &deltas {
                let lat_str = match lat {
                    Some(ms) => format!("{:.1}ms", ms),
                    None => "—".into(),
                };
                let multiplier = if *count > 1.0 {
                    dim(&format!(" (× {:.0})", count))
                } else {
                    String::new()
                };
                println!(
                    "{} {} {} {} {} {}{}",
                    dim(&ts.to_string()),
                    pad(&k.service, 18),
                    pad(&k.method, 6),
                    pad(&k.route, 24),
                    pad(&k.status, 4),
                    lat_str,
                    multiplier,
                );
            }
        }

        prev = current;
        prev_latency = current_latency;
        first = false;

        let total = Duration::from_secs(interval.max(1));
        let start = Instant::now();
        while start.elapsed() < total {
            if stop.load(Ordering::SeqCst) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
    Ok(())
}

fn pad(s: &str, w: usize) -> String {
    if s.len() >= w {
        s.to_string()
    } else {
        format!("{}{}", s, " ".repeat(w - s.len()))
    }
}

// ---------------------------------------------------------------------------
// Exec — fire a request through the proxy.
// ---------------------------------------------------------------------------

fn proxy_url_from_admin(admin: &str) -> Result<String> {
    let mut u = url::Url::parse(admin).context("parsing --admin URL")?;
    if let Some(p) = u.port() {
        let proxy_port = p
            .checked_sub(1)
            .ok_or_else(|| anyhow!("admin port {p} cannot be decremented to derive proxy port"))?;
        u.set_port(Some(proxy_port))
            .map_err(|_| anyhow!("setting proxy port"))?;
    } else {
        // Default convention: admin 9001 → proxy 9000.
        u.set_port(Some(9000))
            .map_err(|_| anyhow!("setting proxy port"))?;
    }
    u.set_path("");
    Ok(u.as_str().trim_end_matches('/').to_string())
}

async fn cmd_exec(
    admin_url: &str,
    path: String,
    method: String,
    body: Option<String>,
    headers: Vec<String>,
    proxy_override: Option<String>,
    max_body: usize,
) -> Result<()> {
    let proxy_base = match proxy_override {
        Some(p) => p.trim_end_matches('/').to_string(),
        None => proxy_url_from_admin(admin_url)?,
    };
    let url = if path.starts_with('/') {
        format!("{proxy_base}{path}")
    } else {
        format!("{proxy_base}/{path}")
    };

    let m = method.to_ascii_uppercase();
    let method_parsed = reqwest::Method::from_bytes(m.as_bytes())
        .with_context(|| format!("invalid method '{m}'"))?;

    let body_bytes: Option<Vec<u8>> = match body {
        Some(s) if s.starts_with('@') => {
            let path = &s[1..];
            Some(fs::read(path).with_context(|| format!("reading body file '{path}'"))?)
        }
        Some(s) => Some(s.into_bytes()),
        None => None,
    };

    let http = reqwest::Client::new();
    let mut req = http.request(method_parsed.clone(), &url);
    for h in &headers {
        let (k, v) = h
            .split_once(':')
            .ok_or_else(|| anyhow!("header '{h}': expected 'Key: Value'"))?;
        req = req.header(k.trim(), v.trim());
    }
    if let Some(b) = body_bytes {
        req = req.body(b);
    }

    let started = Instant::now();
    let resp = req
        .send()
        .await
        .with_context(|| format!("{} {url}", method_parsed))?;
    let elapsed = started.elapsed();
    let status = resp.status();
    let resp_headers = resp.headers().clone();
    let raw = resp.bytes().await.unwrap_or_default();

    // Status line.
    let status_color = |s: reqwest::StatusCode| -> String {
        let n = s.as_u16();
        let s_str = format!("{} {}", n, s.canonical_reason().unwrap_or(""));
        match n {
            200..=299 => green(&s_str),
            300..=399 => yellow(&s_str),
            _ => red(&s_str),
        }
    };
    println!(
        "{} {}  {}  {}",
        bold(&method_parsed.to_string()),
        url,
        status_color(status),
        dim(&format!("({:.1}ms)", elapsed.as_secs_f64() * 1000.0)),
    );

    // Headers (dim).
    let mut hk: Vec<_> = resp_headers.iter().collect();
    hk.sort_by_key(|(k, _)| k.as_str().to_ascii_lowercase());
    for (k, v) in hk {
        let v_str = v.to_str().unwrap_or("<binary>");
        println!("{} {}: {}", dim("│"), dim(k.as_str()), v_str);
    }
    println!();

    // Body — pretty-print JSON when content-type matches; truncate.
    let content_type = resp_headers
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let total_len = raw.len();
    let display: Vec<u8> = if max_body > 0 && total_len > max_body {
        raw[..max_body].to_vec()
    } else {
        raw.to_vec()
    };

    if content_type.contains("application/json") {
        match std::str::from_utf8(&display)
            .ok()
            .and_then(|s| serde_json::from_str::<Json>(s).ok())
        {
            Some(j) => {
                let pp = serde_json::to_string_pretty(&j).unwrap_or_default();
                println!("{pp}");
            }
            None => {
                println!("{}", String::from_utf8_lossy(&display));
            }
        }
    } else {
        println!("{}", String::from_utf8_lossy(&display));
    }
    if max_body > 0 && total_len > max_body {
        println!(
            "{}",
            dim(&format!(
                "(truncated: {} of {} bytes shown — use --max-body 0 for full output)",
                max_body, total_len
            ))
        );
    }

    if !status.is_success() {
        // Reflect HTTP failure as exit code without re-printing.
        bail!("HTTP {}", status.as_u16());
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Entrypoint
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();

    // Color resolution: --no-color > NO_COLOR > tty check (deferred to color_enabled()).
    let no_color_env = std::env::var_os("NO_COLOR").is_some();
    if cli.no_color || no_color_env {
        COLOR_OFF.store(true, Ordering::Relaxed);
    }
    if cli.full_id {
        FULL_ID.store(true, Ordering::Relaxed);
    }

    // Resolve admin URL: --admin > LYNXX_ADMIN > config file > default.
    let local = load_config();
    let admin_url = cli
        .admin
        .clone()
        .or_else(|| local.admin.clone())
        .unwrap_or_else(|| "http://localhost:9001".into());

    // Resolve output format: --output > config file > Table.
    let output = cli.output.unwrap_or_else(|| match local.output.as_deref() {
        Some("json") => OutputFormat::Json,
        Some("yaml") => OutputFormat::Yaml,
        _ => OutputFormat::Table,
    });

    let client = Client::new(&admin_url);

    let result: Result<()> = match cli.command {
        Cmd::Ping => cmd_ping(&client).await,
        Cmd::Status => cmd_status(&client, output).await,
        Cmd::Services(s) => match s {
            ServicesCmd::List => cmd_services_list(&client, output).await,
            ServicesCmd::Get { ident } => cmd_services_get(&client, &ident, output).await,
            ServicesCmd::Create {
                name,
                url,
                retries,
                connect_timeout_ms,
                lb_strategy,
            } => {
                cmd_services_create(
                    &client,
                    name,
                    url,
                    retries,
                    connect_timeout_ms,
                    lb_strategy,
                    output,
                )
                .await
            }
            ServicesCmd::Delete { ident } => cmd_services_delete(&client, &ident).await,
        },
        Cmd::Routes(r) => match r {
            RoutesCmd::List => cmd_routes_list(&client, output).await,
            RoutesCmd::Create {
                service,
                paths,
                methods,
                strip_path,
            } => cmd_routes_create(&client, service, paths, methods, strip_path, output).await,
            RoutesCmd::Delete { id } => cmd_routes_delete(&client, &id).await,
        },
        Cmd::Plugins(p) => match p {
            PluginsCmd::List => cmd_plugins_list(&client, output).await,
            PluginsCmd::Install {
                name,
                service,
                route,
                config,
            } => cmd_plugins_install(&client, name, service, route, config, output).await,
            PluginsCmd::Delete { id } => cmd_plugins_delete(&client, &id).await,
        },
        Cmd::Wasm(w) => match w {
            WasmCmd::Install {
                path,
                name,
                config,
                service,
                route,
                fuel,
                memory_kb,
                wall_ms,
            } => {
                cmd_wasm_install(
                    &client, path, name, config, service, route, fuel, memory_kb, wall_ms, output,
                )
                .await
            }
            WasmCmd::List => cmd_wasm_list(&client, output).await,
            WasmCmd::Remove { id } => cmd_wasm_remove(&client, &id).await,
        },
        Cmd::Scripts(s) => match s {
            ScriptsCmd::Install {
                on_request_cmd,
                on_request_args,
                on_response_cmd,
                on_response_args,
                timeout,
                env,
                max_body_bytes,
                service,
                route,
            } => {
                cmd_scripts_install(
                    &client,
                    on_request_cmd,
                    on_request_args,
                    on_response_cmd,
                    on_response_args,
                    timeout,
                    env,
                    max_body_bytes,
                    service,
                    route,
                    output,
                )
                .await
            }
            ScriptsCmd::List => cmd_scripts_list(&client, output).await,
            ScriptsCmd::Delete { id } => cmd_scripts_delete(&client, &id).await,
        },
        Cmd::Consumers(c) => match c {
            ConsumersCmd::List => cmd_consumers_list(&client, output).await,
            ConsumersCmd::Create {
                username,
                custom_id,
            } => cmd_consumers_create(&client, username, custom_id, output).await,
            ConsumersCmd::Delete { ident } => cmd_consumers_delete(&client, &ident).await,
        },
        Cmd::Targets(t) => match t {
            TargetsCmd::List { service } => cmd_targets_list(&client, service, output).await,
            TargetsCmd::Add {
                service,
                url,
                weight,
            } => cmd_targets_add(&client, service, url, weight, output).await,
            TargetsCmd::Delete { id } => cmd_targets_delete(&client, &id).await,
        },
        Cmd::Health => cmd_health(&client, output).await,
        Cmd::Reload => cmd_reload(&client).await,
        Cmd::Dump => cmd_dump(&client).await,
        Cmd::Apply {
            file,
            dry_run,
            auto_approve,
        } => cmd_apply(&client, file, dry_run, auto_approve).await,
        Cmd::Import {
            spec,
            service_prefix,
            upstream,
            strip_base_path,
            validators,
            mock,
            dry_run,
            auto_approve,
        } => {
            cmd_import(
                &client,
                spec,
                service_prefix,
                upstream,
                strip_base_path,
                validators,
                mock,
                dry_run,
                auto_approve,
            )
            .await
        }
        Cmd::Validate { file } => cmd_validate(&client, file).await,
        Cmd::Watch { resource, interval } => cmd_watch(&client, resource, interval, output).await,
        Cmd::Tail { interval } => cmd_tail(&client, interval).await,
        Cmd::Exec {
            path,
            method,
            body,
            headers,
            proxy,
            max_body,
        } => cmd_exec(&admin_url, path, method, body, headers, proxy, max_body).await,
        Cmd::Record(r) => match r {
            RecordCmd::List {
                since,
                method,
                path_prefix,
                status,
                limit,
            } => cmd_record_list(&client, since, method, path_prefix, status, limit, output).await,
            RecordCmd::Show { id } => cmd_record_show(&client, &id, output).await,
            RecordCmd::Replay { id, against } => {
                cmd_record_replay(&client, &id, against, output).await
            }
            RecordCmd::Delete { id } => cmd_record_delete(&client, &id).await,
        },
        Cmd::Config(c) => cmd_config(c),
        Cmd::Completion { shell } => {
            let mut cmd = Cli::command();
            let bin = cmd.get_name().to_string();
            generate(shell, &mut cmd, bin, &mut io::stdout());
            Ok(())
        }
        Cmd::Start { args } => cmd_start(args),
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            print_error(&e);
            ExitCode::from(1)
        }
    }
}

/// `lynxx start [...]` — replaces the current process with `lynxx-gateway`,
/// forwarding any extra arguments. We look the gateway binary up next to
/// the running CLI binary first, then fall back to PATH so users who
/// installed via the install.sh script get it without extra config.
fn cmd_start(args: Vec<String>) -> Result<()> {
    use std::os::unix::process::CommandExt;
    use std::process::Command;

    let gateway = locate_gateway()?;
    println!("{} Starting Lynxx Gateway...", dim("→"));
    println!("{} Once started, the dashboard will be available at http://localhost:9001", green("✓"));

    let err = Command::new(&gateway).args(&args).exec();
    Err(anyhow::anyhow!(
        "failed to exec {}: {}",
        gateway.display(),
        err
    ))
}

fn locate_gateway() -> Result<PathBuf> {
    if let Ok(self_path) = std::env::current_exe() {
        if let Some(dir) = self_path.parent() {
            let candidate = dir.join("lynxx-gateway");
            if candidate.is_file() {
                return Ok(candidate);
            }
        }
    }
    if let Ok(path_var) = std::env::var("PATH") {
        for dir in std::env::split_paths(&path_var) {
            let candidate = dir.join("lynxx-gateway");
            if candidate.is_file() {
                return Ok(candidate);
            }
        }
    }
    Err(anyhow::anyhow!(
        "lynxx-gateway binary not found next to lynxx or on PATH. \
         Reinstall via `curl -fsSL https://api.lynxx.vexarr.com/install.sh | bash`."
    ))
}

fn print_error(e: &anyhow::Error) {
    let stderr_tty = io::stderr().is_terminal();
    let prefix = if stderr_tty && !COLOR_OFF.load(Ordering::Relaxed) {
        "\x1b[31m\x1b[1m✗\x1b[0m "
    } else {
        "✗ "
    };

    // Pull the head error as the "summary" line; show the chain (if any) as dim continuation.
    let head = e.to_string();
    let mut chain: Vec<String> = Vec::new();
    let mut src = e.source();
    while let Some(s) = src {
        chain.push(s.to_string());
        src = s.source();
    }

    // ApiError's Display already includes the parsed envelope message; show that
    // as the one-liner.
    let _ = writeln!(io::stderr(), "{prefix}{head}");
    if !chain.is_empty() {
        let cont = if stderr_tty && !COLOR_OFF.load(Ordering::Relaxed) {
            "\x1b[2m└─\x1b[0m"
        } else {
            "└─"
        };
        for c in &chain {
            let line = if stderr_tty && !COLOR_OFF.load(Ordering::Relaxed) {
                format!("\x1b[2m{c}\x1b[0m")
            } else {
                c.clone()
            };
            let _ = writeln!(io::stderr(), "  {cont} {line}");
        }
    }
}
