//! Per-request state shuttled between the host and a WASM guest.
//!
//! `HostContext` is stored as the wasmtime `Store` data so every host
//! function callback can reach it via `Caller::data()` /
//! `Caller::data_mut()`. Mutations made by the guest (e.g.
//! `lynxx_set_request_header`) accumulate here, and the plugin's
//! `on_request` / `on_response` glue applies them back to the live
//! `axum::http::Request` / `Response` after the WASM call returns.

/// Logical phase the guest is executing in. Used to gate which getters
/// and setters are valid (e.g. `lynxx_set_request_header` is meaningless
/// during `on_response`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Request,
    Response,
}

#[derive(Debug)]
pub struct HostContext {
    pub phase: Phase,
    pub method: String,
    pub path: String,
    pub request_headers: Vec<(String, String)>,
    pub request_body: Vec<u8>,
    pub response_status: Option<u16>,
    pub response_headers: Vec<(String, String)>,
    pub consumer: Option<String>,
    pub config_json: String,
    /// When set, the host short-circuits the request after the WASM call
    /// returns, synthesizing a response with this status + body.
    pub short_circuit: Option<(u16, Vec<u8>)>,
    /// Seed for the deterministic-per-request `lynxx_random_bytes` PRNG.
    /// The host reseeds this from `rand::thread_rng()` on every request,
    /// so the guest still gets cryptographically-unpredictable bytes
    /// without needing direct entropy access.
    pub rng_seed: u64,
    /// Body-size guardrail: `lynxx_set_request_body` rejects payloads
    /// larger than this. Defaults to 1MB; a future iteration could let
    /// operators tune it via `WasmConfig`.
    pub body_max_bytes: usize,
    /// Wasmtime resource limiter — co-located in the store data so the
    /// `Store::limiter(...)` closure can hand out a `&mut StoreLimits`
    /// borrowed from the same data the host functions read.
    pub limits: wasmtime::StoreLimits,
}

impl HostContext {
    pub fn new(
        phase: Phase,
        config_json: String,
        rng_seed: u64,
        limits: wasmtime::StoreLimits,
    ) -> Self {
        Self {
            phase,
            method: String::new(),
            path: String::new(),
            request_headers: Vec::new(),
            request_body: Vec::new(),
            response_status: None,
            response_headers: Vec::new(),
            consumer: None,
            config_json,
            short_circuit: None,
            rng_seed,
            body_max_bytes: 1024 * 1024,
            limits,
        }
    }

    /// Find a header value by case-insensitive name. Mirrors HTTP
    /// header semantics so a guest written against e.g. `Content-Type`
    /// still hits a header stored as `content-type`.
    pub fn find_header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
        headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    pub fn set_header(headers: &mut Vec<(String, String)>, name: &str, value: &str) {
        if let Some(slot) = headers
            .iter_mut()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
        {
            slot.1 = value.to_string();
        } else {
            headers.push((name.to_string(), value.to_string()));
        }
    }

    pub fn remove_header(headers: &mut Vec<(String, String)>, name: &str) {
        headers.retain(|(k, _)| !k.eq_ignore_ascii_case(name));
    }
}
