//! Integration tests for the OpenAPI-Native feature set.
//!
//! These bypass the admin API and exercise the in-process plugin objects
//! directly. They're intentionally small — the unit tests inside
//! `src/plugins/openapi_*` cover the logic, and the in-binary tests inside
//! the CLI's `openapi_import_tests` cover the spec→state conversion.

use lynx_gateway::error::PluginError;
use lynx_gateway::plugins::openapi_drift::OpenApiDriftPlugin;
use lynx_gateway::plugins::openapi_mock::OpenApiMockPlugin;
use lynx_gateway::plugins::Plugin;

const SPEC: &str = r#"
openapi: 3.0.3
info: { title: ints, version: '1.0' }
paths:
  /things/{id}:
    get:
      responses:
        '200':
          description: ok
          content:
            application/json:
              schema:
                type: object
                required: [id]
                properties:
                  id: { type: integer }
              example:
                id: 99
"#;

fn write_spec() -> (tempfile::TempDir, String) {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("spec.yaml");
    std::fs::write(&p, SPEC).unwrap();
    let path = p.to_string_lossy().to_string();
    (dir, path)
}

#[tokio::test]
async fn mock_returns_spec_example() {
    let (_dir, path) = write_spec();
    let cfg = serde_json::json!({ "spec_path": path });
    let plugin = OpenApiMockPlugin::from_config(&cfg).unwrap();
    let mut req = axum::http::Request::builder()
        .method("GET")
        .uri("/things/abc")
        .body(axum::body::Body::empty())
        .unwrap();
    let res = plugin.on_request(&mut req).await;
    match res {
        Err(PluginError::ShortCircuit { status, body, .. }) => {
            assert_eq!(status, 200);
            let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(v["id"], 99);
        }
        other => panic!("expected short-circuit: {other:?}"),
    }
}

#[tokio::test]
async fn drift_builds_from_spec() {
    let (_dir, path) = write_spec();
    let cfg = serde_json::json!({
        "spec_path": path,
        "sample_rate": 1.0,
        "max_violations_per_minute": 100,
    });
    let plugin = OpenApiDriftPlugin::from_config(&cfg).unwrap();
    assert_eq!(plugin.name(), "openapi-drift");
}
