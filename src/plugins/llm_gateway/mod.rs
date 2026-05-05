//! `llm-gateway` — native multi-provider LLM dispatcher.
//!
//! This is the headline plugin in Lynxx: a single hop that turns the proxy
//! into a smart gateway in front of OpenAI, Anthropic, and self-hosted LLM
//! APIs. It bundles seven capabilities behind one config:
//!
//! 1. **Provider routing** — pick an upstream by model id,
//! 2. **Token counting** — `tiktoken` for OpenAI, `len/4` estimate elsewhere,
//! 3. **Exact-match cache** — sha256 over a canonical request payload,
//! 4. **Prompt-injection guard** — configurable regex blocklist,
//! 5. **PII redaction** — email / SSN / phone / credit card / IP,
//! 6. **Per-consumer token & request rate limits** — rolling 60s buckets,
//! 7. **Multi-provider fallback** — 429/5xx walks the chain,
//! 8. **Cost tracking** — Prometheus `lynxx_llm_*` counters per consumer.
//!
//! Architecturally the plugin is a *short-circuiting* plugin (like
//! `aws-lambda`): on a request to a recognised LLM path it makes the upstream
//! call itself and returns `PluginError::ShortCircuit` with the rendered
//! response. This keeps the proxy hot path untouched and lets the plugin own
//! end-to-end behaviour — fallback retries, cache writes, streaming consumption,
//! response decoration with `X-Lynxx-LLM-*` headers — without surgery on
//! `proxy/handler.rs`.
//!
//! The plugin is path-aware: only `/v1/chat/completions`, `/v1/completions`,
//! `/v1/messages`, and `/v1/embeddings` activate the LLM pipeline. Other paths
//! pass through as no-ops so a route can mix LLM and non-LLM upstreams without
//! the plugin getting in the way.
//!
//! # Limitations (v1)
//!
//! - **Anthropic native `/v1/messages`** with an OpenAI-shaped client request
//!   is not auto-translated. If you target the Anthropic provider you must
//!   send the Anthropic body shape; the plugin will route + add credentials
//!   correctly but won't transmute fields.
//! - **Streaming** is "buffer-then-replay" (see `stream.rs`). Tokens and
//!   metrics are still emitted accurately; latency-to-first-byte is not
//!   improved versus the upstream's own streaming, but bandwidth and policy
//!   correctness are.
//! - **Semantic caching** is exact-match. v2 is on the roadmap (vector ANN).

pub mod buckets;
pub mod cache;
pub mod config;
pub mod cost;
pub mod fallback;
pub mod guard;
pub mod router;
pub mod stream;
pub mod tokens;

#[cfg(test)]
mod integration_tests;

use crate::error::{GatewayError, PluginError};
use crate::plugins::{BoxBody, Plugin};
use async_trait::async_trait;
use axum::body::Body;
use axum::http::{Request, Response};
use http_body_util::BodyExt;
use serde_json::Value;
use std::sync::Arc;
use std::time::Duration;

use buckets::{Buckets, Decision, DenyReason};
use cache::ResponseCache;
use config::LlmGatewayConfig;
use cost::CostTable;
use guard::Guard;
use router::Router;

/// LLM-aware request paths recognised by the plugin. Anything outside this
/// set bypasses the LLM pipeline (the plugin returns `Ok(())` immediately,
/// letting the proxy hot path forward to whatever upstream the route was
/// configured for). The list intentionally matches OpenAI's API surface;
/// Anthropic's `/v1/messages` is included for completeness.
const LLM_PATHS: &[&str] = &[
    "/v1/chat/completions",
    "/v1/completions",
    "/v1/messages",
    "/v1/embeddings",
];

/// Header advertising which provider ultimately served the request (after
/// any fallbacks).
const H_PROVIDER: &str = "x-lynxx-llm-provider";
/// "HIT" or "MISS" for the response cache.
const H_CACHE: &str = "x-lynxx-llm-cache";
/// Counter of fallback hops taken, e.g. "0" for primary, "1" if we fell through
/// once.
const H_FALLBACK: &str = "x-lynxx-llm-fallback";
/// Tokens billed.
const H_TOKENS_IN: &str = "x-lynxx-llm-tokens-in";
const H_TOKENS_OUT: &str = "x-lynxx-llm-tokens-out";
const H_COST: &str = "x-lynxx-llm-cost-usd";
/// Count of PII matches replaced before forwarding.
const H_REDACTED: &str = "x-lynxx-llm-redacted";

pub struct LlmGatewayPlugin {
    router: Router,
    cache: Arc<ResponseCache>,
    guard: Arc<Guard>,
    buckets: Arc<Buckets>,
    cost: CostTable,
    http: reqwest::Client,
}

impl LlmGatewayPlugin {
    pub fn from_config(cfg: &serde_json::Value) -> Result<Self, GatewayError> {
        let parsed: LlmGatewayConfig = serde_json::from_value(cfg.clone())
            .map_err(|e| GatewayError::BadRequest(format!("llm-gateway config: {e}")))?;
        let router = Router::build(&parsed).map_err(GatewayError::BadRequest)?;

        let cache = ResponseCache::new(parsed.cache.enabled, parsed.cache.ttl_seconds);
        if parsed.cache.strategy != "exact" {
            tracing::warn!(
                strategy = %parsed.cache.strategy,
                "llm-gateway: cache strategy other than `exact` is reserved for v2; falling back to exact"
            );
        }

        let guard = Arc::new(Guard::build(
            parsed.guard.block_prompt_injection,
            &parsed.guard.injection_patterns,
            &parsed.guard.redact_pii,
        ));

        let buckets = Buckets::new(
            parsed.rate_limit.enabled,
            parsed.rate_limit.tokens_per_minute_per_consumer,
            parsed.rate_limit.requests_per_minute_per_consumer,
        );

        let cost = CostTable::new(parsed.cost_tracking.enabled, parsed.cost_tracking.model_costs);

        // 120s is generous; matched to the default OpenAI socket timeout.
        // Pool defaults are fine — one shared client across providers; the
        // hostnames cluster well enough that the connection cache is useful.
        let http = reqwest::Client::builder()
            .pool_idle_timeout(Duration::from_secs(90))
            .timeout(Duration::from_secs(120))
            .build()
            .map_err(|e| GatewayError::BadRequest(format!("llm-gateway http: {e}")))?;

        Ok(Self {
            router,
            cache,
            guard,
            buckets,
            cost,
            http,
        })
    }

    fn is_llm_path(path: &str) -> bool {
        LLM_PATHS.iter().any(|p| path.starts_with(p))
    }
}

#[async_trait]
impl Plugin for LlmGatewayPlugin {
    fn name(&self) -> &str {
        "llm-gateway"
    }

    /// The whole pipeline — guard, redact, rate-limit, cache, dispatch, count,
    /// price, decorate — runs from `on_request`. We short-circuit the proxy
    /// in either direction:
    ///
    /// - Cache hit → ShortCircuit with the cached response,
    /// - Guard reject → ShortCircuit with 400,
    /// - Rate limit reject → ShortCircuit with 429,
    /// - Successful dispatch → ShortCircuit with the upstream response.
    ///
    /// Non-LLM paths return `Ok(())` and let the normal proxy flow continue.
    async fn on_request(&self, req: &mut Request<Body>) -> Result<(), PluginError> {
        let path = req.uri().path().to_string();
        if !Self::is_llm_path(&path) {
            return Ok(());
        }

        // Pull the consumer label set by upstream auth plugins. The convention
        // is `X-Consumer-Username`; missing means anonymous, which still gets
        // its own bucket — anonymous abuse is real.
        let consumer = req
            .headers()
            .get("x-consumer-username")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("anonymous")
            .to_string();

        // Buffer the request body. LLM payloads are typically a few KB to a
        // few hundred KB; if your prompts exceed that, you should be using
        // file uploads or vector retrieval anyway. We cap at 4 MiB to keep
        // memory bounded.
        let body = std::mem::replace(req.body_mut(), Body::empty());
        let collected = body
            .collect()
            .await
            .map_err(|e| PluginError::Internal(format!("llm-gateway body: {e}")))?
            .to_bytes();
        if collected.len() > 4 * 1024 * 1024 {
            return Err(PluginError::PayloadTooLarge(
                "llm-gateway: request body exceeds 4 MiB".into(),
            ));
        }
        let mut json: Value = match serde_json::from_slice(&collected) {
            Ok(v) => v,
            Err(_) => {
                // Not JSON — let it pass through. The upstream will reject it
                // with a more specific error than we could synthesise. We
                // restore the body and exit the LLM path.
                *req.body_mut() = Body::from(collected);
                return Ok(());
            }
        };

        let model = json
            .get("model")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();
        let messages_owned: Vec<Value> = json
            .get("messages")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let stream_requested = json
            .get("stream")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
            || req
                .headers()
                .get(axum::http::header::ACCEPT)
                .and_then(|v| v.to_str().ok())
                .map(|v| v.contains("text/event-stream"))
                .unwrap_or(false);

        // -- Guard: prompt injection -----------------------------------
        if let Some(_pat) = self.guard.check_injection(&messages_owned) {
            metrics::counter!(
                "lynxx_llm_guard_total",
                "consumer" => consumer.clone(),
                "action" => "injection_blocked",
            )
            .increment(1);
            let body = serde_json::to_vec(&serde_json::json!({
                "error": {
                    "code": "prompt_injection_detected",
                    "message": "request blocked by llm-gateway prompt-injection guard"
                }
            }))
            .unwrap();
            return Err(PluginError::ShortCircuit {
                status: 400,
                headers: vec![("Content-Type".into(), "application/json".into())],
                body,
            });
        }

        // -- Guard: PII redaction --------------------------------------
        let mut messages = messages_owned;
        let redacted = self.guard.redact_pii(&mut messages);
        if redacted > 0 {
            metrics::counter!(
                "lynxx_llm_guard_total",
                "consumer" => consumer.clone(),
                "action" => "pii_redacted",
            )
            .increment(redacted);
            // Write the redacted messages back into the JSON body before
            // forwarding. Using `serde_json::Value::Array` + `Object::insert`
            // preserves all sibling fields the caller may have set.
            if let Some(obj) = json.as_object_mut() {
                obj.insert("messages".to_string(), Value::Array(messages.clone()));
            }
        }

        // -- Token estimation for the rate limiter ---------------------
        let est_input = tokens::count_messages(&model, &messages) as u64;

        // -- Rate limit: per-consumer tokens + requests ----------------
        match self.buckets.check(&consumer, est_input) {
            Decision::Allow => {}
            Decision::Deny { reason, retry_after } => {
                let code = match reason {
                    DenyReason::Tokens => "token_rate_limit_exceeded",
                    DenyReason::Requests => "request_rate_limit_exceeded",
                };
                let body = serde_json::to_vec(&serde_json::json!({
                    "error": { "code": code, "retry_after": retry_after }
                }))
                .unwrap();
                return Err(PluginError::ShortCircuit {
                    status: 429,
                    headers: vec![
                        ("Content-Type".into(), "application/json".into()),
                        ("Retry-After".into(), retry_after.to_string()),
                    ],
                    body,
                });
            }
        }

        // -- Provider pick + dispatch chain ----------------------------
        let primary = self.router.pick(&model).name.clone();
        let dispatch_chain = self.router.dispatch_order(&primary);

        // -- Cache lookup. We skip caching for streaming requests because
        //    the cached body would be a sequence of SSE frames — replaying
        //    them is correct but mostly defeats the latency benefit, and
        //    upstreams sometimes vary frame timing. Off-policy for v1.
        let cache_key = self.cache.key(&model, &json);
        if !stream_requested {
            if let Some(hit) = self.cache.get(&cache_key) {
                metrics::counter!(
                    "lynxx_llm_cache_total",
                    "consumer" => consumer.clone(),
                    "result" => "hit",
                )
                .increment(1);
                let mut headers = hit.headers.clone();
                headers.push((H_CACHE.into(), "HIT".into()));
                headers.push((H_PROVIDER.into(), primary.clone()));
                if redacted > 0 {
                    headers.push((H_REDACTED.into(), redacted.to_string()));
                }
                return Err(PluginError::ShortCircuit {
                    status: hit.status,
                    headers,
                    body: hit.body,
                });
            }
            metrics::counter!(
                "lynxx_llm_cache_total",
                "consumer" => consumer.clone(),
                "result" => "miss",
            )
            .increment(1);
        }

        // Re-serialize the (possibly PII-redacted) body for the upstream.
        let outbound = serde_json::to_vec(&json)
            .map_err(|e| PluginError::Internal(format!("llm-gateway encode: {e}")))?;

        // Snapshot client headers (we'll filter inside `dispatch`).
        let forwarded_headers: Vec<(String, String)> = req
            .headers()
            .iter()
            .filter_map(|(k, v)| v.to_str().ok().map(|s| (k.as_str().to_string(), s.to_string())))
            .collect();

        // -- Dispatch with fallback ------------------------------------
        let consumer_for_metric = consumer.clone();
        let result = fallback::dispatch(
            &self.http,
            &dispatch_chain,
            &path,
            &reqwest::Method::POST,
            &forwarded_headers,
            &outbound,
            |from, to| {
                metrics::counter!(
                    "lynxx_llm_provider_fallback_total",
                    "from_provider" => from.to_string(),
                    "to_provider" => to.to_string(),
                )
                .increment(1);
            },
        )
        .await
        .map_err(PluginError::Internal)?;

        // -- Token / cost accounting -----------------------------------
        let content_type = result
            .headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("content-type"))
            .map(|(_, v)| v.as_str());

        let (input_tokens, output_tokens) = if stream::is_event_stream(content_type) {
            let usage = stream::analyze_sse_body(&model, &result.body);
            let in_t = usage.upstream_prompt_tokens.unwrap_or(est_input);
            let out_t = usage
                .upstream_completion_tokens
                .unwrap_or(usage.completion_tokens);
            (in_t, out_t)
        } else {
            // Try to parse JSON and pull `usage`; fall back to estimate over
            // the response text.
            let parsed: Value =
                serde_json::from_slice(&result.body).unwrap_or(Value::Null);
            match tokens::extract_usage(&parsed) {
                Some(u) => u,
                None => {
                    // Estimate over choices[].message.content (OpenAI shape)
                    // or content[].text (Anthropic shape).
                    let mut out = 0u64;
                    if let Some(choices) = parsed.get("choices").and_then(|c| c.as_array()) {
                        for c in choices {
                            if let Some(t) = c
                                .pointer("/message/content")
                                .and_then(|v| v.as_str())
                            {
                                out += tokens::count_text(&model, t) as u64;
                            }
                        }
                    }
                    if let Some(content) = parsed.get("content").and_then(|c| c.as_array()) {
                        for c in content {
                            if let Some(t) = c.get("text").and_then(|v| v.as_str()) {
                                out += tokens::count_text(&model, t) as u64;
                            }
                        }
                    }
                    (est_input, out)
                }
            }
        };

        // Adjust the rate-limit bucket for any delta between estimate and
        // actual input + the (post-hoc) output tokens.
        let extra = input_tokens.saturating_sub(est_input) + output_tokens;
        self.buckets.deduct(&consumer_for_metric, extra);

        // -- Metrics ---------------------------------------------------
        let provider_label = result.provider.clone();
        let model_label = model.clone();
        metrics::counter!(
            "lynxx_llm_input_tokens_total",
            "consumer" => consumer_for_metric.clone(),
            "model" => model_label.clone(),
            "provider" => provider_label.clone(),
        )
        .increment(input_tokens);
        metrics::counter!(
            "lynxx_llm_output_tokens_total",
            "consumer" => consumer_for_metric.clone(),
            "model" => model_label.clone(),
            "provider" => provider_label.clone(),
        )
        .increment(output_tokens);
        let dollars = self.cost.dollars(&model, input_tokens, output_tokens);
        if self.cost.enabled() {
            // metrics::counter accepts an integer, so we scale to micro-cents
            // (1e6) for non-zero precision then divide back at PromQL time.
            // The published metric name still says `dollars_total` and
            // the Grafana tile divides by 1e6 — this is the convention
            // adopted across the rest of the gateway.
            let micro = (dollars * 1_000_000.0).round() as u64;
            metrics::counter!(
                "lynxx_llm_cost_dollars_total",
                "consumer" => consumer_for_metric,
                "model" => model_label,
                "provider" => provider_label,
            )
            .increment(micro);
        }

        // -- Build the short-circuit response --------------------------
        let mut headers = result.headers.clone();
        headers.push((H_PROVIDER.into(), result.provider.clone()));
        headers.push((H_CACHE.into(), "MISS".into()));
        headers.push((H_FALLBACK.into(), result.fallback_count.to_string()));
        headers.push((H_TOKENS_IN.into(), input_tokens.to_string()));
        headers.push((H_TOKENS_OUT.into(), output_tokens.to_string()));
        if self.cost.enabled() {
            headers.push((H_COST.into(), format!("{dollars:.6}")));
        }
        if redacted > 0 {
            headers.push((H_REDACTED.into(), redacted.to_string()));
        }

        // -- Cache write (only for non-stream successful responses) ----
        if !stream_requested {
            self.cache.put(
                cache_key,
                result.status,
                result.headers.clone(),
                result.body.clone(),
            );
        }

        Err(PluginError::ShortCircuit {
            status: result.status,
            headers,
            body: result.body,
        })
    }

    async fn on_response(&self, _resp: &mut Response<BoxBody>) -> Result<(), PluginError> {
        // All work is done in `on_request` because the plugin short-circuits.
        // If a future version splits the hot path (e.g. true streaming), the
        // response-side hook is where token counting would move.
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build(cfg: serde_json::Value) -> LlmGatewayPlugin {
        LlmGatewayPlugin::from_config(&cfg).expect("config builds")
    }

    #[test]
    fn config_with_minimum_fields() {
        let p = build(serde_json::json!({
            "providers": [{"name":"openai","base_url":"https://api.openai.com/v1"}]
        }));
        assert_eq!(p.name(), "llm-gateway");
    }

    #[test]
    fn rejects_no_providers() {
        let err = LlmGatewayPlugin::from_config(&serde_json::json!({"providers":[]})).err();
        assert!(err.is_some());
    }

    #[test]
    fn rejects_unknown_default_provider() {
        let err = LlmGatewayPlugin::from_config(&serde_json::json!({
            "providers":[{"name":"a","base_url":"http://x"}],
            "default_provider":"b"
        })).err();
        assert!(err.is_some());
    }

    #[test]
    fn is_llm_path_recognises_endpoints() {
        assert!(LlmGatewayPlugin::is_llm_path("/v1/chat/completions"));
        assert!(LlmGatewayPlugin::is_llm_path("/v1/messages"));
        assert!(LlmGatewayPlugin::is_llm_path("/v1/embeddings"));
        assert!(!LlmGatewayPlugin::is_llm_path("/healthz"));
        assert!(!LlmGatewayPlugin::is_llm_path("/"));
    }
}
