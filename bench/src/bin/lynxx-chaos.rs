//! lynxx-chaos — battle-test orchestrator.
//!
//! Each subcommand runs one chaos scenario against a live Lynxx gateway. The
//! `all` subcommand runs every scenario sequentially with shorter defaults so
//! the whole suite finishes in well under fifteen minutes.
//!
//! Scenarios live in `lynxx_bench::chaos::*` (one file each). This binary is
//! pure CLI dispatch.

use std::time::Duration;

use anyhow::Result;
use clap::{Parser, Subcommand};

use lynxx_bench::chaos::{
    burst, churn, malformed, memory, methods, mixed_bodies, pg_restart, plugin_chain, slowloris,
    ws_flood, AdminClient, Args as ChaosArgs, Report,
};

#[derive(Parser, Debug)]
#[command(name = "lynxx-chaos", version, about = "Chaos / soak / battle-test suite for the Lynxx gateway")]
struct Cli {
    /// Proxy listener URL (data plane).
    #[arg(long, default_value = "http://127.0.0.1:9000", global = true)]
    proxy: String,

    /// Admin API URL.
    #[arg(long, default_value = "http://127.0.0.1:9001", global = true)]
    admin: String,

    /// Optional admin token (`Authorization: Bearer ...`).
    #[arg(long, global = true)]
    admin_token: Option<String>,

    /// Mock upstream URL — start one with `lynxx-mock-upstream` first.
    #[arg(long, default_value = "http://127.0.0.1:19090", global = true)]
    mock_upstream: String,

    /// Postgres docker container name (only required for `pg-restart`).
    #[arg(long, global = true)]
    pg_container: Option<String>,

    /// Override per-scenario duration in seconds.
    #[arg(long, global = true)]
    duration_secs: Option<u64>,

    /// Override per-scenario RPS.
    #[arg(long, global = true)]
    rps: Option<u64>,

    /// Less verbose progress output.
    #[arg(long, global = true)]
    quiet: bool,

    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// 70/20/9/1 GET / 1KB / 100KB / 5MB body mix.
    MixedBodies,
    /// 100 byte-per-300ms slow clients alongside fast traffic.
    Slowloris,
    /// Mixed methods + status code distribution from mock upstream.
    Methods,
    /// 200 -> 2000 -> 200 RPS spike survivability.
    Burst,
    /// Service create/delete churn during steady traffic.
    Churn,
    /// Stack 5 plugins and verify the chain.
    PluginChain,
    /// 500 WebSocket connections at 100 msg/sec each.
    WsFlood,
    /// 10-min soak; sample RSS, check for leaks.
    Memory,
    /// docker restart <pg> mid-traffic.
    PgRestart,
    /// 1000 raw malformed TCP requests.
    Malformed,
    /// Run all 10 scenarios sequentially with short defaults.
    All,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();
    let args = ChaosArgs {
        proxy_url: cli.proxy.clone(),
        admin_url: cli.admin.clone(),
        admin_token: cli.admin_token.clone(),
        mock_upstream_url: cli.mock_upstream.clone(),
        pg_container: cli.pg_container.clone(),
        duration: cli.duration_secs.map(Duration::from_secs),
        rps: cli.rps,
        quiet: cli.quiet,
    };
    let admin = AdminClient::new(args.admin_url.clone(), args.admin_token.clone())?;
    admin
        .ping()
        .await
        .map_err(|e| anyhow::anyhow!("gateway admin not reachable at {}: {e}", args.admin_url))?;

    let reports = match cli.command {
        Cmd::MixedBodies => vec![mixed_bodies::run(&admin, &args).await?],
        Cmd::Slowloris => vec![slowloris::run(&admin, &args).await?],
        Cmd::Methods => vec![methods::run(&admin, &args).await?],
        Cmd::Burst => vec![burst::run(&admin, &args).await?],
        Cmd::Churn => vec![churn::run(&admin, &args).await?],
        Cmd::PluginChain => vec![plugin_chain::run(&admin, &args).await?],
        Cmd::WsFlood => vec![ws_flood::run(&admin, &args).await?],
        Cmd::Memory => vec![memory::run(&admin, &args).await?],
        Cmd::PgRestart => vec![pg_restart::run(&admin, &args).await?],
        Cmd::Malformed => vec![malformed::run(&admin, &args).await?],
        Cmd::All => run_all(&admin, &args).await,
    };

    print_summary(&reports);
    if reports.iter().any(|r| !r.passed) {
        std::process::exit(1);
    }
    Ok(())
}

/// Run every scenario sequentially with shorter defaults so the whole suite
/// finishes in <15min. `pg_restart` is skipped if no container is supplied.
async fn run_all(admin: &AdminClient, base: &ChaosArgs) -> Vec<Report> {
    let mut reports = Vec::new();

    // Each scenario gets its own Args clone so individual overrides don't leak.
    let short = |secs: u64| ChaosArgs {
        duration: Some(Duration::from_secs(secs)),
        ..base.clone()
    };

    macro_rules! step {
        ($name:literal, $fut:expr) => {{
            tracing::info!(scenario = $name, "starting");
            match $fut.await {
                Ok(r) => {
                    tracing::info!(scenario = $name, passed = r.passed, "done");
                    reports.push(r);
                }
                Err(e) => {
                    tracing::error!(scenario = $name, error = %e, "scenario errored");
                    reports.push(Report::fail($name, format!("errored: {e}")));
                }
            }
        }};
    }

    step!("mixed-bodies", mixed_bodies::run(admin, &short(60)));
    step!("slowloris", slowloris::run(admin, &short(30)));
    step!("methods", methods::run(admin, &short(60)));
    step!("burst", burst::run(admin, &short(0))); // burst uses its own 3-phase timeline
    step!("churn", churn::run(admin, &short(60)));
    step!("plugin-chain", plugin_chain::run(admin, &short(30)));
    step!("ws-flood", ws_flood::run(admin, &short(30)));
    step!("memory", memory::run(admin, &short(120))); // 2 min in `all` mode
    if base.pg_container.is_some() {
        step!("pg-restart", pg_restart::run(admin, &short(60)));
    } else {
        reports.push(
            Report::fail("pg-restart", "skipped: --pg-container not supplied")
                .detail("pass --pg-container <name> to include this scenario"),
        );
        // Mark as passed so `all` can still succeed without docker; the detail
        // line preserves the skip in the output.
        let last = reports.last_mut().unwrap();
        last.passed = true;
        last.summary = "skipped (no --pg-container)".into();
    }
    step!("malformed", malformed::run(admin, &short(0)));

    reports
}

fn print_summary(reports: &[Report]) {
    println!();
    println!("==== chaos suite summary ====");
    let mut passed = 0;
    for r in reports {
        let mark = if r.passed { "\u{2713}" } else { "\u{2717}" };
        println!("{mark} {:<14} {}", r.scenario, r.summary);
        for line in &r.details {
            println!("    {line}");
        }
        if r.passed {
            passed += 1;
        }
    }
    println!();
    println!("{passed}/{} scenarios passed", reports.len());
}
