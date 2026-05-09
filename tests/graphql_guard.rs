//! Integration tests for the `graphql-guard` plugin.
//!
//! These tests exercise the public Plugin contract: build the plugin from a
//! JSON config, synthesise a request via `axum::http::Request`, call
//! `on_request`, and assert. We avoid spinning up the full gateway — the
//! plugin is self-contained.

use axum::body::Body;
use axum::http::Request;
use lynxx_gateway::error::PluginError;
use lynxx_gateway::plugins::graphql_guard::GraphqlGuardPlugin;
use lynxx_gateway::plugins::Plugin;
use serde_json::json;

fn req_with_body(body: serde_json::Value) -> Request<Body> {
    let bytes = serde_json::to_vec(&body).unwrap();
    let mut r = Request::new(Body::from(bytes));
    r.headers_mut()
        .insert("content-type", "application/json".parse().unwrap());
    *r.uri_mut() = "/graphql".parse().unwrap();
    r
}

fn build(cfg: serde_json::Value) -> GraphqlGuardPlugin {
    GraphqlGuardPlugin::from_config(&cfg).expect("plugin builds")
}

async fn assert_short_circuit(err: PluginError, status: u16, code: &str) {
    match err {
        PluginError::ShortCircuit {
            status: s, body, ..
        } => {
            assert_eq!(s, status, "status mismatch");
            let bytes = body.into_bytes().await;
            let parsed: serde_json::Value = serde_json::from_slice(&bytes).expect("body is JSON");
            let actual = parsed
                .pointer("/errors/0/extensions/code")
                .and_then(|v| v.as_str())
                .unwrap_or("<missing>");
            assert_eq!(actual, code, "extensions.code mismatch");
        }
        other => panic!("expected ShortCircuit, got {other:?}"),
    }
}

#[tokio::test]
async fn depth_limit_triggers() {
    let p = build(json!({ "max_depth": 3 }));
    let mut req = req_with_body(json!({
        "query": "{ a { b { c { d { e } } } } }"
    }));
    let err = p.on_request(&mut req).await.unwrap_err();
    assert_short_circuit(err, 400, "QUERY_TOO_DEEP").await;
}

#[tokio::test]
async fn complexity_limit_triggers() {
    let p = build(json!({
        "max_complexity": 5,
        "field_costs": { "users": 5, "posts": 3 },
        "default_field_cost": 1
    }));
    let mut req = req_with_body(json!({
        "query": "{ users { id } posts { id } }"
    }));
    let err = p.on_request(&mut req).await.unwrap_err();
    assert_short_circuit(err, 400, "QUERY_TOO_COMPLEX").await;
}

#[tokio::test]
async fn alias_count_triggers() {
    let p = build(json!({ "max_aliases": 2 }));
    let mut req = req_with_body(json!({
        "query": "{ a: x b: x c: x d: x }"
    }));
    let err = p.on_request(&mut req).await.unwrap_err();
    assert_short_circuit(err, 400, "TOO_MANY_ALIASES").await;
}

#[tokio::test]
async fn persisted_query_lookup_hit() {
    let p = build(json!({
        "persisted_queries": {
            "hash1": "{ user { id name } }"
        }
    }));
    let mut req = req_with_body(json!({
        "extensions": { "persistedQuery": { "sha256Hash": "hash1" } }
    }));
    let res = p.on_request(&mut req).await;
    assert!(res.is_ok(), "persisted query hit should pass: {res:?}");
    // Body should now contain the resolved query.
    let bytes = http_body_util::BodyExt::collect(std::mem::take(req.body_mut()))
        .await
        .unwrap()
        .to_bytes();
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        v.get("query").and_then(|v| v.as_str()),
        Some("{ user { id name } }")
    );
    assert_eq!(
        req.headers()
            .get("x-lynxx-graphql-persisted")
            .and_then(|v| v.to_str().ok()),
        Some("true")
    );
}

#[tokio::test]
async fn persisted_query_miss_strict() {
    let p = build(json!({
        "persisted_queries_only": true,
        "persisted_queries": {}
    }));
    let mut req = req_with_body(json!({
        "extensions": { "persistedQuery": { "sha256Hash": "missing" } }
    }));
    let err = p.on_request(&mut req).await.unwrap_err();
    assert_short_circuit(err, 400, "PERSISTED_QUERY_NOT_FOUND").await;
}

#[tokio::test]
async fn introspection_blocked_in_production() {
    let p = build(json!({
        "production_mode": true,
        "introspection_disabled_in_production": true
    }));
    let mut req = req_with_body(json!({
        "query": "{ __schema { types { name } } }"
    }));
    let err = p.on_request(&mut req).await.unwrap_err();
    assert_short_circuit(err, 403, "INTROSPECTION_DISABLED").await;
}

#[tokio::test]
async fn per_field_rate_limit() {
    let p = build(json!({
        "field_rate_limits": [
            { "field": "deleteUser", "requests_per_second": 1, "key_strategy": "consumer" }
        ]
    }));

    // First request: allowed.
    let mut req1 = req_with_body(json!({
        "query": "mutation { deleteUser(id: \"1\") { ok } }"
    }));
    req1.headers_mut()
        .insert("x-consumer-username", "alice".parse().unwrap());
    assert!(p.on_request(&mut req1).await.is_ok());

    // Hammer 10 more — second onward should rate-limit.
    let mut limited = false;
    for _ in 0..10 {
        let mut req = req_with_body(json!({
            "query": "mutation { deleteUser(id: \"1\") { ok } }"
        }));
        req.headers_mut()
            .insert("x-consumer-username", "alice".parse().unwrap());
        if let Err(e) = p.on_request(&mut req).await {
            if matches!(&e, PluginError::ShortCircuit { status: 429, .. }) {
                limited = true;
                break;
            }
        }
    }
    assert!(limited, "expected at least one rate-limit hit");
}

#[tokio::test]
async fn mutations_blocked_for_anonymous() {
    let p = build(json!({ "block_mutations_for_anonymous": true }));
    let mut req = req_with_body(json!({
        "query": "mutation { deleteUser(id: \"1\") { ok } }"
    }));
    let err = p.on_request(&mut req).await.unwrap_err();
    assert_short_circuit(err, 401, "ANONYMOUS_MUTATIONS_BLOCKED").await;
}

#[tokio::test]
async fn mutations_allowed_for_named_consumer() {
    let p = build(json!({ "block_mutations_for_anonymous": true }));
    let mut req = req_with_body(json!({
        "query": "mutation { deleteUser(id: \"1\") { ok } }"
    }));
    req.headers_mut()
        .insert("x-consumer-username", "alice".parse().unwrap());
    assert!(p.on_request(&mut req).await.is_ok());
}

#[tokio::test]
async fn operation_name_required() {
    let p = build(json!({ "operation_name_required": true }));
    let mut req = req_with_body(json!({
        "query": "query { user { id } }"
    }));
    let err = p.on_request(&mut req).await.unwrap_err();
    assert_short_circuit(err, 400, "OPERATION_NAME_REQUIRED").await;
}

#[tokio::test]
async fn operation_name_present_passes() {
    let p = build(json!({ "operation_name_required": true }));
    let mut req = req_with_body(json!({
        "query": "query Foo { user { id } }"
    }));
    assert!(p.on_request(&mut req).await.is_ok());
    assert_eq!(
        req.headers()
            .get("x-lynxx-graphql-operation")
            .and_then(|v| v.to_str().ok()),
        Some("Foo")
    );
}

#[tokio::test]
async fn non_graphql_request_passes_through() {
    let p = build(json!({ "max_depth": 1 }));
    // application/json body but no GraphQL fields — should pass through.
    let mut req = req_with_body(json!({ "hello": "world" }));
    let res = p.on_request(&mut req).await;
    assert!(res.is_ok(), "non-graphql json should pass: {res:?}");
    assert!(req
        .headers()
        .get("x-lynxx-graphql-operation-type")
        .is_none());
}

#[tokio::test]
async fn non_json_request_passes_through() {
    let p = build(json!({}));
    let mut req: Request<Body> = Request::new(Body::from("hello"));
    req.headers_mut()
        .insert("content-type", "text/plain".parse().unwrap());
    assert!(p.on_request(&mut req).await.is_ok());
}

#[tokio::test]
async fn parse_error_returns_graphql_parse_failed() {
    let p = build(json!({}));
    let mut req = req_with_body(json!({ "query": "this is not valid graphql {{{" }));
    let err = p.on_request(&mut req).await.unwrap_err();
    assert_short_circuit(err, 400, "GRAPHQL_PARSE_FAILED").await;
}

#[tokio::test]
async fn injects_metadata_headers_on_success() {
    let p = build(json!({}));
    let mut req = req_with_body(json!({
        "query": "query GetUser { user { id name } }"
    }));
    assert!(p.on_request(&mut req).await.is_ok());
    assert_eq!(
        req.headers()
            .get("x-lynxx-graphql-operation-type")
            .and_then(|v| v.to_str().ok()),
        Some("query")
    );
    assert!(req.headers().get("x-lynxx-graphql-complexity").is_some());
    assert!(req.headers().get("x-lynxx-graphql-depth").is_some());
}
