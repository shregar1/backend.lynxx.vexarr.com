//! End-to-end integration test for the admin API.
//!
//! Spawns `lynx-gateway` as a child process pointed at a freshly minted
//! temp directory + `config.toml`, waits for `:admin/status` to come up,
//! then exercises a service create/get/delete cycle. The Postgres variant
//! runs the same flow against a Postgres URL and is gated on the
//! `LYNXX_TEST_PG=1` environment variable so it stays out of the way of
//! plain `cargo test` runs.

use std::io::Write;
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::{Duration, Instant};

use serde_json::json;
use tokio::process::{Child, Command};

/// Drop guard that kills the gateway subprocess so a panicking test
/// never leaks a port-bound process.
struct GatewayGuard {
    child: Option<Child>,
}

impl GatewayGuard {
    fn new(child: Child) -> Self {
        Self { child: Some(child) }
    }
}

impl Drop for GatewayGuard {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            // start_kill is non-async and safe to call from Drop; the
            // OS reaps it when the test process exits.
            let _ = child.start_kill();
        }
    }
}

fn pick_free_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    port
}

fn write_config(dir: &PathBuf, proxy_port: u16, admin_port: u16, db_url: &str) -> PathBuf {
    let path = dir.join("config.toml");
    let mut f = std::fs::File::create(&path).expect("create config");
    writeln!(
        f,
        "[gateway]\n\
         proxy_port = {proxy_port}\n\
         admin_port = {admin_port}\n\
         database_url = \"{db_url}\"\n\
         seed_from_config = false\n\
         health_check_interval_ms = 60000\n\
         health_circuit_failure_threshold = 3\n\
         health_circuit_success_threshold = 2\n\
         health_circuit_open_duration_ms = 30000\n"
    )
    .unwrap();
    path
}

async fn wait_for_status(admin_port: u16, timeout: Duration) -> bool {
    let url = format!("http://127.0.0.1:{admin_port}/status");
    let client = reqwest::Client::new();
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if let Ok(r) = client.get(&url).send().await {
            if r.status().is_success() {
                return true;
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    false
}

async fn run_admin_cycle(admin_port: u16) {
    let client = reqwest::Client::new();
    let base = format!("http://127.0.0.1:{admin_port}");

    // POST /services
    let resp = client
        .post(format!("{base}/services"))
        .json(&json!({
            "name": "it-test-svc",
            "url": "http://127.0.0.1:1",
            "retries": 1,
            "connect_timeout_ms": 1000,
        }))
        .send()
        .await
        .expect("post");
    assert!(
        resp.status().is_success(),
        "create_service status: {}",
        resp.status()
    );
    let body: serde_json::Value = resp.json().await.expect("json");
    let id = body
        .get("id")
        .and_then(|v| v.as_str())
        .expect("service id in response")
        .to_string();

    // GET /services/:id
    let resp = client
        .get(format!("{base}/services/{id}"))
        .send()
        .await
        .expect("get");
    assert!(
        resp.status().is_success(),
        "get_service status: {}",
        resp.status()
    );
    let body: serde_json::Value = resp.json().await.expect("json");
    assert_eq!(
        body.get("name").and_then(|v| v.as_str()),
        Some("it-test-svc")
    );

    // DELETE /services/:id
    let resp = client
        .delete(format!("{base}/services/{id}"))
        .send()
        .await
        .expect("delete");
    assert!(
        resp.status().is_success(),
        "delete_service status: {}",
        resp.status()
    );

    // confirm gone
    let resp = client
        .get(format!("{base}/services/{id}"))
        .send()
        .await
        .expect("get");
    assert_eq!(resp.status().as_u16(), 404, "service should be gone");
}

async fn launch_gateway(db_url: &str) -> (GatewayGuard, u16) {
    let dir = tempfile::tempdir().expect("tempdir");
    // Leak the TempDir so the gateway can keep using files in it until the
    // child is killed at end of test. The OS reclaims the temp dir on exit.
    let dir_path = dir.keep();
    let proxy_port = pick_free_port();
    let admin_port = pick_free_port();
    let config = write_config(&dir_path, proxy_port, admin_port, db_url);

    let bin = env!("CARGO_BIN_EXE_lynx-gateway");
    let child = Command::new(bin)
        .env("LYNXX_CONFIG", &config)
        .env("RUST_LOG", "lynx_gateway=warn")
        .current_dir(&dir_path)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .expect("spawn lynx-gateway");

    let guard = GatewayGuard::new(child);
    let up = wait_for_status(admin_port, Duration::from_secs(30)).await;
    assert!(
        up,
        "gateway admin port {admin_port} did not respond in time"
    );
    (guard, admin_port)
}

#[tokio::test]
async fn admin_api_sqlite_lifecycle() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db_path = dir.path().join("test.db");
    let db_url = format!("sqlite://{}?mode=rwc", db_path.display());
    let (_guard, admin_port) = launch_gateway(&db_url).await;
    run_admin_cycle(admin_port).await;
}

#[tokio::test]
async fn admin_api_postgres_lifecycle() {
    if std::env::var("LYNXX_TEST_PG").ok().as_deref() != Some("1") {
        eprintln!("skipping postgres test: set LYNXX_TEST_PG=1 to enable");
        return;
    }
    let db_url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://lynxx:lynxx@localhost:5432/lynxx".to_string());
    let (_guard, admin_port) = launch_gateway(&db_url).await;
    run_admin_cycle(admin_port).await;
}
