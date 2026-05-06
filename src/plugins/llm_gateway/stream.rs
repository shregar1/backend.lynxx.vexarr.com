//! SSE (Server-Sent Events) helpers for streaming LLM responses.
//!
//! v1 streaming approach: when the upstream returns
//! `Content-Type: text/event-stream` we forward the body bytes to the client
//! verbatim and parse a *copy* synchronously to count completion tokens.
//! Because the proxy hot path materialises the full response body before the
//! `on_response` plugin chain runs (see `proxy/handler.rs`), our streaming
//! support is "all-at-once" in the gateway: the upstream stream is consumed
//! in full, then the response is replayed to the client. This matches the
//! existing architectural contract; a future optimisation could re-thread
//! true incremental forwarding, but every commercial gateway today (Kong AI
//! Gateway, Portkey, Helicone) does the same thing for the sake of policy
//! evaluation.
//!
//! The parser walks `data: {...}\n\n` frames and extracts:
//! - OpenAI shape: `choices[].delta.content` (string)
//! - Anthropic shape: `delta.text` for `content_block_delta` events
//! - Final `[DONE]` sentinel, ignored
//!
//! Tokens are counted via [`super::tokens::count_text`] using the request's
//! model id.

use super::tokens;
use serde_json::Value;

/// Output of stream-body analysis.
#[derive(Debug, Clone, Default)]
pub struct StreamUsage {
    pub completion_tokens: u64,
    /// Some upstreams (OpenAI when `stream_options: {include_usage: true}`)
    /// emit a final `usage` block — we prefer it over our own count when
    /// available.
    pub upstream_prompt_tokens: Option<u64>,
    pub upstream_completion_tokens: Option<u64>,
}

/// Detect "is this an SSE stream" by content-type. The header value can have
/// parameters appended (`text/event-stream; charset=utf-8`), so we match the
/// prefix.
pub fn is_event_stream(content_type: Option<&str>) -> bool {
    content_type
        .map(|c| {
            c.trim_start()
                .to_ascii_lowercase()
                .starts_with("text/event-stream")
        })
        .unwrap_or(false)
}

/// Parse a fully-buffered SSE body and return token counts. Both providers
/// emit `data: <json>\n\n` per event; we ignore comment lines (`:`),
/// `event:` lines, and the `[DONE]` sentinel.
pub fn analyze_sse_body(model: &str, body: &[u8]) -> StreamUsage {
    let mut out = StreamUsage::default();
    let s = std::str::from_utf8(body).unwrap_or("");
    for raw_line in s.split('\n') {
        let line = raw_line.trim_end_matches('\r');
        let Some(payload) = line
            .strip_prefix("data: ")
            .or_else(|| line.strip_prefix("data:"))
        else {
            continue;
        };
        let payload = payload.trim();
        if payload.is_empty() || payload == "[DONE]" {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(payload) else {
            continue;
        };

        // OpenAI streaming: choices[].delta.content
        if let Some(choices) = v.get("choices").and_then(|c| c.as_array()) {
            for ch in choices {
                if let Some(delta) = ch.get("delta") {
                    if let Some(text) = delta.get("content").and_then(|t| t.as_str()) {
                        out.completion_tokens += tokens::count_text(model, text) as u64;
                    }
                }
            }
        }

        // Anthropic streaming: type=="content_block_delta" with delta.text
        if v.get("type").and_then(|t| t.as_str()) == Some("content_block_delta") {
            if let Some(text) = v
                .get("delta")
                .and_then(|d| d.get("text"))
                .and_then(|t| t.as_str())
            {
                out.completion_tokens += tokens::count_text(model, text) as u64;
            }
        }

        // Final usage block (OpenAI w/ stream_options, Anthropic message_stop)
        if let Some(usage) = v
            .get("usage")
            .or_else(|| v.get("message").and_then(|m| m.get("usage")))
        {
            if let Some(p) = usage
                .get("prompt_tokens")
                .or_else(|| usage.get("input_tokens"))
                .and_then(|x| x.as_u64())
            {
                out.upstream_prompt_tokens = Some(p);
            }
            if let Some(c) = usage
                .get("completion_tokens")
                .or_else(|| usage.get("output_tokens"))
                .and_then(|x| x.as_u64())
            {
                out.upstream_completion_tokens = Some(c);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_event_stream_content_type() {
        assert!(is_event_stream(Some("text/event-stream")));
        assert!(is_event_stream(Some("text/event-stream; charset=utf-8")));
        assert!(!is_event_stream(Some("application/json")));
        assert!(!is_event_stream(None));
    }

    #[test]
    fn counts_openai_stream_tokens() {
        let body = b"data: {\"choices\":[{\"delta\":{\"content\":\"hello\"}}]}\n\n\
                    data: {\"choices\":[{\"delta\":{\"content\":\" world\"}}]}\n\n\
                    data: [DONE]\n\n";
        let u = analyze_sse_body("gpt-4o", body);
        assert!(u.completion_tokens >= 2);
    }

    #[test]
    fn counts_anthropic_stream_tokens() {
        let body = b"data: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"hi\"}}\n\n\
                    data: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\" there\"}}\n\n";
        let u = analyze_sse_body("claude-3-5-sonnet-20240620", body);
        assert!(u.completion_tokens >= 1);
    }

    #[test]
    fn picks_up_upstream_usage_block() {
        let body =
            b"data: {\"choices\":[],\"usage\":{\"prompt_tokens\":7,\"completion_tokens\":13}}\n\n";
        let u = analyze_sse_body("gpt-4o", body);
        assert_eq!(u.upstream_prompt_tokens, Some(7));
        assert_eq!(u.upstream_completion_tokens, Some(13));
    }
}
