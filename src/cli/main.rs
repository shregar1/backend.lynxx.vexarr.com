//! lynxx — declarative CLI for the Lynxx gateway admin API.
//!
//! Talks HTTP to the gateway's admin port (default `http://localhost:9001`).
//! Produces human-friendly tables by default and JSON/YAML on request, plus
//! an `apply`/`dump` pair for idempotent declarative state management.
//!
//! Aesthetic: monochrome ANSI (bold + dim only), narrow columns, no fancy
//! borders. Errors go to stderr with a "✗" prefix when stdout is a TTY.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io::{self, IsTerminal, Read, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{anyhow, bail, Context, Result};
use chrono::{DateTime, Utc};
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
    propagate_version = true
)]
struct Cli {
    /// Admin API base URL
    #[arg(long, env = "LYNXX_ADMIN", default_value = "http://localhost:9001", global = true)]
    admin: String,

    /// Output format for machine-readable commands
    #[arg(long, value_enum, default_value_t = OutputFormat::Table, global = true)]
    output: OutputFormat,

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
    Apply {
        /// Path to YAML state file ("-" for stdin)
        #[arg(short = 'f', long = "file")]
        file: PathBuf,
        /// Print the diff plan but do not mutate
        #[arg(long)]
        dry_run: bool,
    },
    /// Generate shell completion script
    Completion {
        /// Target shell
        shell: Shell,
    },
}

#[derive(Subcommand, Debug)]
enum ServicesCmd {
    List,
    Get { ident: String },
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
    Delete { ident: String },
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
    Delete { id: String },
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
    Delete { id: String },
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
    Delete { ident: String },
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
    Delete { id: String },
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
    healthy_targets: u32,
    unhealthy_targets: u32,
    circuit_state: String,
    last_check: Option<DateTime<Utc>>,
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

fn default_retries() -> u32 { 3 }
fn default_timeout() -> u64 { 5000 }
fn default_weight() -> u32 { 1 }
fn default_lb() -> String { "round-robin".into() }
fn default_scope() -> String { "global".into() }

// ---------------------------------------------------------------------------
// HTTP client
// ---------------------------------------------------------------------------

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
        let resp = self.http.get(&url).send().await
            .with_context(|| format!("GET {url}"))?;
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            bail!("GET {url} → {status}: {}", text.trim());
        }
        serde_json::from_str(&text).with_context(|| format!("decoding response from {url}"))
    }

    async fn post<B: Serialize, T: for<'de> Deserialize<'de>>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<T> {
        let url = format!("{}{}", self.base, path);
        let resp = self.http.post(&url).json(body).send().await
            .with_context(|| format!("POST {url}"))?;
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            bail!("POST {url} → {status}: {}", text.trim());
        }
        serde_json::from_str(&text).with_context(|| format!("decoding response from {url}"))
    }

    async fn post_empty(&self, path: &str) -> Result<Json> {
        let url = format!("{}{}", self.base, path);
        let resp = self.http.post(&url).send().await
            .with_context(|| format!("POST {url}"))?;
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            bail!("POST {url} → {status}: {}", text.trim());
        }
        if text.is_empty() {
            return Ok(Json::Null);
        }
        Ok(serde_json::from_str(&text).unwrap_or(Json::String(text)))
    }

    async fn delete(&self, path: &str) -> Result<()> {
        let url = format!("{}{}", self.base, path);
        let resp = self.http.delete(&url).send().await
            .with_context(|| format!("DELETE {url}"))?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            bail!("DELETE {url} → {status}: {}", text.trim());
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Output helpers — bold/dim ANSI when stdout is a terminal.
// ---------------------------------------------------------------------------

fn tty() -> bool {
    io::stdout().is_terminal()
}

fn bold(s: &str) -> String {
    if tty() { format!("\x1b[1m{s}\x1b[0m") } else { s.to_string() }
}

fn dim(s: &str) -> String {
    if tty() { format!("\x1b[2m{s}\x1b[0m") } else { s.to_string() }
}

/// Print a simple monospace table. `headers` is rendered dim; rows are normal.
/// Column widths fit the longest cell in each column.
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
    // Header
    let mut out = String::new();
    for (i, h) in headers.iter().enumerate() {
        let pad = " ".repeat(widths[i].saturating_sub(h.len()));
        if i + 1 < ncol {
            out.push_str(&format!("{}{}  ", h, pad));
        } else {
            out.push_str(h);
        }
    }
    println!("{}", dim(&out));
    // Rows
    for row in rows {
        let mut line = String::new();
        for (i, cell) in row.iter().enumerate().take(ncol) {
            let pad = " ".repeat(widths[i].saturating_sub(cell.len()));
            if i + 1 < ncol {
                line.push_str(&format!("{}{}  ", cell, pad));
            } else {
                line.push_str(cell);
            }
        }
        println!("{}", line);
    }
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

fn short_id(id: &Uuid) -> String {
    let s = id.to_string();
    s[..8].to_string()
}

// ---------------------------------------------------------------------------
// Resolve helpers — accept name OR id for human convenience.
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
    // Allow short-prefix id match.
    let matches: Vec<&Service> = services.iter()
        .filter(|s| s.id.to_string().starts_with(ident))
        .collect();
    match matches.len() {
        1 => Ok(matches[0].clone()),
        0 => Err(anyhow!("no service matched '{ident}'")),
        _ => Err(anyhow!("'{ident}' is ambiguous ({} matches)", matches.len())),
    }
}

async fn resolve_consumer(client: &Client, ident: &str) -> Result<Consumer> {
    let consumers: Vec<Consumer> = client.get("/consumers").await?;
    if let Ok(uuid) = Uuid::parse_str(ident) {
        if let Some(c) = consumers.iter().find(|c| c.id == uuid) {
            return Ok(c.clone());
        }
    }
    consumers.iter()
        .find(|c| c.username == ident)
        .cloned()
        .ok_or_else(|| anyhow!("no consumer matched '{ident}'"))
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

async fn cmd_ping(client: &Client) -> Result<()> {
    let s: StatusResponse = client.get("/status").await?;
    println!(
        "{} lynxx {} — {} (up {})",
        bold("✓"),
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
    let s: StatusResponse = client.get("/status").await?;
    emit(&s, fmt)
}

fn humanize_secs(s: i64) -> String {
    if s < 60 { format!("{s}s") }
    else if s < 3600 { format!("{}m{}s", s / 60, s % 60) }
    else if s < 86_400 { format!("{}h{}m", s / 3600, (s % 3600) / 60) }
    else { format!("{}d{}h", s / 86_400, (s % 86_400) / 3600) }
}

async fn cmd_services_list(client: &Client, fmt: OutputFormat) -> Result<()> {
    let services: Vec<Service> = client.get("/services").await?;
    if fmt != OutputFormat::Table {
        return emit(&services, fmt);
    }
    let rows: Vec<Vec<String>> = services.iter().map(|s| {
        vec![
            short_id(&s.id),
            s.name.clone(),
            s.url.clone(),
            s.retries.to_string(),
            format!("{}ms", s.connect_timeout_ms),
            s.lb_strategy.clone().unwrap_or_else(|| "round-robin".into()),
        ]
    }).collect();
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
        "name": name,
        "url": url,
        "retries": retries,
        "connect_timeout_ms": connect_timeout_ms,
    });
    if let Some(lb) = lb_strategy {
        body["lb_strategy"] = Json::String(lb);
    }
    let svc: Service = client.post("/services", &body).await?;
    if fmt == OutputFormat::Table {
        println!("{} created service {} ({})", bold("✓"), svc.name, short_id(&svc.id));
    } else {
        emit(&svc, fmt)?;
    }
    Ok(())
}

async fn cmd_services_delete(client: &Client, ident: &str) -> Result<()> {
    let svc = resolve_service(client, ident).await?;
    client.delete(&format!("/services/{}", svc.id)).await?;
    println!("{} deleted service {} ({})", bold("✓"), svc.name, short_id(&svc.id));
    Ok(())
}

async fn cmd_routes_list(client: &Client, fmt: OutputFormat) -> Result<()> {
    let routes: Vec<Route> = client.get("/routes").await?;
    if fmt != OutputFormat::Table {
        return emit(&routes, fmt);
    }
    // Build id→name map for nicer rendering.
    let services: Vec<Service> = client.get("/services").await.unwrap_or_default();
    let by_id: HashMap<Uuid, String> = services.iter().map(|s| (s.id, s.name.clone())).collect();
    let rows: Vec<Vec<String>> = routes.iter().map(|r| {
        vec![
            short_id(&r.id),
            by_id.get(&r.service_id).cloned().unwrap_or_else(|| short_id(&r.service_id)),
            r.paths.join(","),
            if r.methods.is_empty() { "*".into() } else { r.methods.join(",") },
            if r.strip_path { "yes".into() } else { "no".into() },
        ]
    }).collect();
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
        "paths": paths,
        "methods": methods,
        "service_id": svc.id,
        "strip_path": strip_path,
    });
    let route: Route = client.post("/routes", &body).await?;
    if fmt == OutputFormat::Table {
        println!(
            "{} created route {} → {} (paths: {})",
            bold("✓"),
            short_id(&route.id),
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
    println!("{} deleted route {}", bold("✓"), short_id(&uuid));
    Ok(())
}

async fn cmd_plugins_list(client: &Client, fmt: OutputFormat) -> Result<()> {
    let plugins: Vec<PluginInstance> = client.get("/plugins").await?;
    if fmt != OutputFormat::Table {
        return emit(&plugins, fmt);
    }
    let services: Vec<Service> = client.get("/services").await.unwrap_or_default();
    let by_id: HashMap<Uuid, String> = services.iter().map(|s| (s.id, s.name.clone())).collect();
    let rows: Vec<Vec<String>> = plugins.iter().map(|p| {
        let scope = match (p.service_id, p.route_id) {
            (Some(s), _) => format!("service:{}", by_id.get(&s).cloned().unwrap_or_else(|| short_id(&s))),
            (_, Some(r)) => format!("route:{}", short_id(&r)),
            _ => "global".into(),
        };
        vec![
            short_id(&p.id),
            p.name.clone(),
            scope,
            if p.enabled { "yes".into() } else { "no".into() },
        ]
    }).collect();
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
    let cfg: Json = serde_json::from_str(&config)
        .context("--config must be a valid JSON object")?;
    let service_id = if let Some(s) = service {
        Some(resolve_service(client, &s).await?.id)
    } else { None };
    let route_id = match route {
        Some(r) => Some(Uuid::parse_str(&r).context("--route must be a UUID")?),
        None => None,
    };
    let body = serde_json::json!({
        "name": name,
        "service_id": service_id,
        "route_id": route_id,
        "enabled": true,
        "config": cfg,
    });
    let plugin: PluginInstance = client.post("/plugins", &body).await?;
    if fmt == OutputFormat::Table {
        println!("{} installed plugin {} ({})", bold("✓"), plugin.name, short_id(&plugin.id));
    } else {
        emit(&plugin, fmt)?;
    }
    Ok(())
}

async fn cmd_plugins_delete(client: &Client, id: &str) -> Result<()> {
    let uuid = Uuid::parse_str(id).context("plugin id must be a UUID")?;
    client.delete(&format!("/plugins/{uuid}")).await?;
    println!("{} deleted plugin {}", bold("✓"), short_id(&uuid));
    Ok(())
}

async fn cmd_consumers_list(client: &Client, fmt: OutputFormat) -> Result<()> {
    let consumers: Vec<Consumer> = client.get("/consumers").await?;
    if fmt != OutputFormat::Table {
        return emit(&consumers, fmt);
    }
    let rows: Vec<Vec<String>> = consumers.iter().map(|c| {
        vec![
            short_id(&c.id),
            c.username.clone(),
            c.custom_id.clone().unwrap_or_default(),
            c.api_key.clone().map(|k| {
                let n = k.len().min(8);
                format!("{}…", &k[..n])
            }).unwrap_or_default(),
        ]
    }).collect();
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
        println!("{} created consumer {} ({})", bold("✓"), consumer.username, short_id(&consumer.id));
    } else {
        emit(&consumer, fmt)?;
    }
    Ok(())
}

async fn cmd_consumers_delete(client: &Client, ident: &str) -> Result<()> {
    let consumer = resolve_consumer(client, ident).await?;
    client.delete(&format!("/consumers/{}", consumer.id)).await?;
    println!("{} deleted consumer {} ({})", bold("✓"), consumer.username, short_id(&consumer.id));
    Ok(())
}

async fn cmd_targets_list(client: &Client, service: Option<String>, fmt: OutputFormat) -> Result<()> {
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
    let rows: Vec<Vec<String>> = targets.iter().map(|t| {
        vec![
            short_id(&t.id),
            by_id.get(&t.service_id).cloned().unwrap_or_else(|| short_id(&t.service_id)),
            t.url.clone(),
            t.weight.to_string(),
            if t.enabled { "yes".into() } else { "no".into() },
        ]
    }).collect();
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
    let target: Target = client.post(&format!("/services/{}/targets", svc.id), &body).await?;
    if fmt == OutputFormat::Table {
        println!("{} added target {} → {}", bold("✓"), target.url, svc.name);
    } else {
        emit(&target, fmt)?;
    }
    Ok(())
}

async fn cmd_targets_delete(client: &Client, id: &str) -> Result<()> {
    let uuid = Uuid::parse_str(id).context("target id must be a UUID")?;
    client.delete(&format!("/targets/{uuid}")).await?;
    println!("{} deleted target {}", bold("✓"), short_id(&uuid));
    Ok(())
}

async fn cmd_health(client: &Client, fmt: OutputFormat) -> Result<()> {
    let health: Vec<ServiceHealth> = client.get("/health").await?;
    if fmt != OutputFormat::Table {
        return emit(&health, fmt);
    }
    let services: Vec<Service> = client.get("/services").await.unwrap_or_default();
    let by_id: HashMap<Uuid, String> = services.iter().map(|s| (s.id, s.name.clone())).collect();
    let rows: Vec<Vec<String>> = health.iter().map(|h| {
        vec![
            by_id.get(&h.service_id).cloned().unwrap_or_else(|| short_id(&h.service_id)),
            h.healthy_targets.to_string(),
            h.unhealthy_targets.to_string(),
            h.circuit_state.clone(),
            h.last_check.map(|t| t.to_rfc3339()).unwrap_or_else(|| "—".into()),
        ]
    }).collect();
    print_table(&["SERVICE", "HEALTHY", "UNHEALTHY", "CIRCUIT", "LAST_CHECK"], &rows);
    Ok(())
}

async fn cmd_reload(client: &Client) -> Result<()> {
    let v = client.post_empty("/reload").await?;
    println!("{} reload triggered", bold("✓"));
    if !v.is_null() {
        println!("{}", dim(&serde_json::to_string_pretty(&v)?));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Dump: snapshot the gateway into the declarative schema and emit YAML.
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
        targets_by_svc.entry(t.service_id).or_default().push(DeclTarget {
            url: t.url.clone(),
            weight: t.weight,
        });
    }

    let decl_services: Vec<DeclService> = services.iter().map(|s| DeclService {
        name: s.name.clone(),
        url: s.url.clone(),
        retries: s.retries,
        connect_timeout_ms: s.connect_timeout_ms,
        lb_strategy: s.lb_strategy.clone().unwrap_or_else(default_lb),
        targets: targets_by_svc.remove(&s.id).unwrap_or_default(),
    }).collect();

    let decl_routes: Vec<DeclRoute> = routes.iter().map(|r| DeclRoute {
        service: svc_by_id.get(&r.service_id)
            .map(|s| s.name.clone())
            .unwrap_or_else(|| r.service_id.to_string()),
        paths: r.paths.clone(),
        methods: r.methods.clone(),
        strip_path: r.strip_path,
    }).collect();

    let decl_plugins: Vec<DeclPlugin> = plugins.iter().map(|p| {
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
    }).collect();

    let state = DeclarativeState {
        services: decl_services,
        routes: decl_routes,
        plugins: decl_plugins,
    };
    print!("{}", serde_yaml::to_string(&state)?);
    Ok(())
}

// ---------------------------------------------------------------------------
// Apply: read a YAML state file and reconcile the gateway to match.
//
// Semantics: upsert-by-name for services and (service, paths) for routes;
// missing items are deleted. Plugins are reconciled by (name, scope-key).
// ---------------------------------------------------------------------------

async fn cmd_apply(client: &Client, file: PathBuf, dry_run: bool) -> Result<()> {
    let raw = if file == PathBuf::from("-") {
        let mut buf = String::new();
        io::stdin().read_to_string(&mut buf)?;
        buf
    } else {
        fs::read_to_string(&file)
            .with_context(|| format!("reading {}", file.display()))?
    };
    let desired: DeclarativeState = serde_yaml::from_str(&raw)
        .context("parsing state YAML")?;

    // Snapshot current state.
    let cur_services: Vec<Service> = client.get("/services").await?;
    let cur_routes: Vec<Route> = client.get("/routes").await?;
    let cur_plugins: Vec<PluginInstance> = client.get("/plugins").await?;
    let cur_targets: Vec<Target> = client.get("/targets").await.unwrap_or_default();

    let svc_by_name: HashMap<String, &Service> =
        cur_services.iter().map(|s| (s.name.clone(), s)).collect();
    let want_svc_names: std::collections::HashSet<&str> =
        desired.services.iter().map(|s| s.name.as_str()).collect();

    let mut plan: Vec<String> = Vec::new();

    // ---- services: upsert ------------------------------------------------
    for ds in &desired.services {
        match svc_by_name.get(ds.name.as_str()) {
            Some(existing) => {
                let needs_update = existing.url != ds.url
                    || existing.retries != ds.retries
                    || existing.connect_timeout_ms != ds.connect_timeout_ms
                    || existing.lb_strategy.as_deref().unwrap_or("round-robin") != ds.lb_strategy;
                if needs_update {
                    plan.push(format!("update service {}", ds.name));
                    if !dry_run {
                        let body = serde_json::json!({
                            "name": ds.name,
                            "url": ds.url,
                            "retries": ds.retries,
                            "connect_timeout_ms": ds.connect_timeout_ms,
                            "lb_strategy": ds.lb_strategy,
                        });
                        let url = format!("{}/services/{}", client.base, existing.id);
                        let resp = client.http.put(&url).json(&body).send().await?;
                        if !resp.status().is_success() {
                            bail!("PUT {url} → {}: {}", resp.status(), resp.text().await.unwrap_or_default());
                        }
                    }
                }
            }
            None => {
                plan.push(format!("create service {}", ds.name));
                if !dry_run {
                    let body = serde_json::json!({
                        "name": ds.name,
                        "url": ds.url,
                        "retries": ds.retries,
                        "connect_timeout_ms": ds.connect_timeout_ms,
                        "lb_strategy": ds.lb_strategy,
                    });
                    let _: Service = client.post("/services", &body).await?;
                }
            }
        }
    }

    // Re-snapshot services so we can resolve names → ids for the rest.
    let services_now: Vec<Service> = if dry_run {
        cur_services.clone()
    } else {
        client.get("/services").await?
    };
    let svc_id_by_name: HashMap<String, Uuid> =
        services_now.iter().map(|s| (s.name.clone(), s.id)).collect();

    // ---- targets: replace per service ------------------------------------
    let mut targets_by_svc: HashMap<Uuid, Vec<&Target>> = HashMap::new();
    for t in &cur_targets {
        targets_by_svc.entry(t.service_id).or_default().push(t);
    }
    for ds in &desired.services {
        let svc_id = match svc_id_by_name.get(&ds.name) {
            Some(id) => *id,
            None if dry_run => continue,
            None => bail!("internal: service {} not found post-create", ds.name),
        };
        let existing = targets_by_svc.remove(&svc_id).unwrap_or_default();
        let want: BTreeMap<String, u32> = ds.targets.iter()
            .map(|t| (t.url.clone(), t.weight)).collect();
        let have: BTreeMap<String, (Uuid, u32)> = existing.iter()
            .map(|t| (t.url.clone(), (t.id, t.weight))).collect();

        for (url, weight) in &want {
            match have.get(url) {
                Some((_, w)) if w == weight => {}
                Some((tid, _)) => {
                    plan.push(format!("update target {}={}", url, weight));
                    if !dry_run {
                        let body = serde_json::json!({ "weight": weight });
                        let resp = client.http
                            .put(&format!("{}/targets/{}", client.base, tid))
                            .json(&body).send().await?;
                        if !resp.status().is_success() {
                            bail!("update target {tid}: {}", resp.status());
                        }
                    }
                }
                None => {
                    plan.push(format!("add target {} → {}", url, ds.name));
                    if !dry_run {
                        let body = serde_json::json!({ "url": url, "weight": weight, "enabled": true });
                        let _: Target = client.post(&format!("/services/{}/targets", svc_id), &body).await?;
                    }
                }
            }
        }
        for (url, (tid, _)) in &have {
            if !want.contains_key(url) {
                plan.push(format!("delete target {}", url));
                if !dry_run {
                    client.delete(&format!("/targets/{}", tid)).await?;
                }
            }
        }
    }

    // ---- routes: identify by (service, paths-set) ------------------------
    fn route_key(svc: &str, paths: &[String]) -> String {
        let mut p = paths.to_vec();
        p.sort();
        format!("{svc}|{}", p.join(","))
    }
    let svc_name_by_id: HashMap<Uuid, String> =
        services_now.iter().map(|s| (s.id, s.name.clone())).collect();
    let cur_route_keys: HashMap<String, &Route> = cur_routes.iter()
        .map(|r| {
            let svc = svc_name_by_id.get(&r.service_id).cloned()
                .unwrap_or_else(|| r.service_id.to_string());
            (route_key(&svc, &r.paths), r)
        })
        .collect();
    let want_route_keys: std::collections::HashSet<String> = desired.routes.iter()
        .map(|r| route_key(&r.service, &r.paths))
        .collect();

    for dr in &desired.routes {
        let key = route_key(&dr.service, &dr.paths);
        if cur_route_keys.contains_key(&key) {
            continue; // route already exists with matching shape
        }
        plan.push(format!("create route {} → {}", dr.paths.join(","), dr.service));
        if dry_run { continue; }
        let svc_id = svc_id_by_name.get(&dr.service)
            .ok_or_else(|| anyhow!("route references unknown service '{}'", dr.service))?;
        let body = serde_json::json!({
            "paths": dr.paths,
            "methods": dr.methods,
            "service_id": svc_id,
            "strip_path": dr.strip_path,
        });
        let _: Route = client.post("/routes", &body).await?;
    }
    for (key, route) in &cur_route_keys {
        if !want_route_keys.contains(key) {
            plan.push(format!("delete route {}", route.paths.join(",")));
            if !dry_run {
                client.delete(&format!("/routes/{}", route.id)).await?;
            }
        }
    }

    // ---- plugins: identify by (name, scope-key) --------------------------
    fn plugin_key(name: &str, scope: &str, target: &str) -> String {
        format!("{name}|{scope}|{target}")
    }
    let route_id_by_key: HashMap<String, Uuid> = cur_routes.iter().map(|r| {
        let svc = svc_name_by_id.get(&r.service_id).cloned()
            .unwrap_or_else(|| r.service_id.to_string());
        (route_key(&svc, &r.paths), r.id)
    }).collect();

    let cur_plugin_keys: HashMap<String, &PluginInstance> = cur_plugins.iter().map(|p| {
        let (scope, target) = match (p.service_id, p.route_id) {
            (Some(s), _) => (
                "service",
                svc_name_by_id.get(&s).cloned().unwrap_or_else(|| s.to_string()),
            ),
            (_, Some(r)) => {
                // find route by id, render its service|paths key for symmetry
                let k = cur_routes.iter().find(|rt| rt.id == r).map(|rt| {
                    let svc = svc_name_by_id.get(&rt.service_id).cloned()
                        .unwrap_or_else(|| rt.service_id.to_string());
                    format!("{}|{}", svc, {
                        let mut ps = rt.paths.clone(); ps.sort(); ps.join(";")
                    })
                }).unwrap_or_else(|| r.to_string());
                ("route", k)
            }
            _ => ("global", String::new()),
        };
        (plugin_key(&p.name, scope, &target), p)
    }).collect();

    let mut want_plugin_keys: std::collections::HashSet<String> = Default::default();
    for dp in &desired.plugins {
        let (scope, target) = match dp.scope.as_str() {
            "service" => {
                let s = dp.service.as_deref()
                    .ok_or_else(|| anyhow!("plugin {} scope=service requires `service:`", dp.name))?;
                ("service", s.to_string())
            }
            "route" => {
                let r = dp.route.as_deref()
                    .ok_or_else(|| anyhow!("plugin {} scope=route requires `route:` (paths joined by ';')", dp.name))?;
                // route value is "p1;p2"; produce normalized key as "<svc>|<sorted-paths>"
                // we don't know the service here directly — match by paths only.
                let mut ps: Vec<String> = r.split(';').map(|s| s.to_string()).collect();
                ps.sort();
                let joined = ps.join(";");
                // try to find a route whose paths match
                let svc = cur_routes.iter().find(|rt| {
                    let mut got = rt.paths.clone(); got.sort();
                    got.join(";") == joined
                }).and_then(|rt| svc_name_by_id.get(&rt.service_id).cloned())
                    .unwrap_or_else(|| "?".into());
                ("route", format!("{svc}|{joined}"))
            }
            "global" => ("global", String::new()),
            other => bail!("unknown plugin scope '{other}'"),
        };
        let key = plugin_key(&dp.name, scope, &target);
        want_plugin_keys.insert(key.clone());

        if !cur_plugin_keys.contains_key(&key) {
            plan.push(format!("install plugin {} ({}{})", dp.name, scope,
                if target.is_empty() { String::new() } else { format!(":{}", target) }));
            if dry_run { continue; }
            let (service_id, route_id) = match scope {
                "service" => (svc_id_by_name.get(&target).copied(), None),
                "route" => {
                    // route key was "<svc>|<sorted-paths>"
                    let rid = route_id_by_key.get(&target).copied();
                    (None, rid)
                }
                _ => (None, None),
            };
            let body = serde_json::json!({
                "name": dp.name,
                "service_id": service_id,
                "route_id": route_id,
                "enabled": true,
                "config": dp.config,
            });
            let _: PluginInstance = client.post("/plugins", &body).await?;
        }
    }
    for (key, p) in &cur_plugin_keys {
        if !want_plugin_keys.contains(key) {
            plan.push(format!("delete plugin {} ({})", p.name, short_id(&p.id)));
            if !dry_run {
                client.delete(&format!("/plugins/{}", p.id)).await?;
            }
        }
    }

    // ---- services: delete missing (after routes/plugins are gone) --------
    for s in &cur_services {
        if !want_svc_names.contains(s.name.as_str()) {
            plan.push(format!("delete service {}", s.name));
            if !dry_run {
                client.delete(&format!("/services/{}", s.id)).await?;
            }
        }
    }

    if plan.is_empty() {
        println!("{} nothing to do", bold("✓"));
    } else {
        let header = if dry_run { "plan (dry-run)" } else { "applied" };
        println!("{} {}:", bold("✓"), header);
        for line in &plan {
            println!("  {}", line);
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Entrypoint
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    let client = Client::new(&cli.admin);

    let result: Result<()> = match cli.command {
        Cmd::Ping => cmd_ping(&client).await,
        Cmd::Status => cmd_status(&client, cli.output).await,
        Cmd::Services(s) => match s {
            ServicesCmd::List => cmd_services_list(&client, cli.output).await,
            ServicesCmd::Get { ident } => cmd_services_get(&client, &ident, cli.output).await,
            ServicesCmd::Create { name, url, retries, connect_timeout_ms, lb_strategy } =>
                cmd_services_create(&client, name, url, retries, connect_timeout_ms, lb_strategy, cli.output).await,
            ServicesCmd::Delete { ident } => cmd_services_delete(&client, &ident).await,
        },
        Cmd::Routes(r) => match r {
            RoutesCmd::List => cmd_routes_list(&client, cli.output).await,
            RoutesCmd::Create { service, paths, methods, strip_path } =>
                cmd_routes_create(&client, service, paths, methods, strip_path, cli.output).await,
            RoutesCmd::Delete { id } => cmd_routes_delete(&client, &id).await,
        },
        Cmd::Plugins(p) => match p {
            PluginsCmd::List => cmd_plugins_list(&client, cli.output).await,
            PluginsCmd::Install { name, service, route, config } =>
                cmd_plugins_install(&client, name, service, route, config, cli.output).await,
            PluginsCmd::Delete { id } => cmd_plugins_delete(&client, &id).await,
        },
        Cmd::Consumers(c) => match c {
            ConsumersCmd::List => cmd_consumers_list(&client, cli.output).await,
            ConsumersCmd::Create { username, custom_id } =>
                cmd_consumers_create(&client, username, custom_id, cli.output).await,
            ConsumersCmd::Delete { ident } => cmd_consumers_delete(&client, &ident).await,
        },
        Cmd::Targets(t) => match t {
            TargetsCmd::List { service } => cmd_targets_list(&client, service, cli.output).await,
            TargetsCmd::Add { service, url, weight } =>
                cmd_targets_add(&client, service, url, weight, cli.output).await,
            TargetsCmd::Delete { id } => cmd_targets_delete(&client, &id).await,
        },
        Cmd::Health => cmd_health(&client, cli.output).await,
        Cmd::Reload => cmd_reload(&client).await,
        Cmd::Dump => cmd_dump(&client).await,
        Cmd::Apply { file, dry_run } => cmd_apply(&client, file, dry_run).await,
        Cmd::Completion { shell } => {
            let mut cmd = Cli::command();
            let bin = cmd.get_name().to_string();
            generate(shell, &mut cmd, bin, &mut io::stdout());
            Ok(())
        }
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            let prefix = if io::stderr().is_terminal() { "\x1b[1m✗\x1b[0m " } else { "✗ " };
            let _ = writeln!(io::stderr(), "{prefix}{e:#}");
            ExitCode::from(1)
        }
    }
}
