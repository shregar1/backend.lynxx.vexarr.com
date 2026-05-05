//! Redaction helpers used before a `Recording` is persisted.
//!
//! Two strategies are supported:
//!
//! - `redact_headers` matches header names case-insensitively and replaces
//!   the value with `[redacted]`. Used for `Authorization`, cookies, etc.
//! - `redact_body_paths` walks a `serde_json::Value` and replaces any
//!   object key whose name matches one of the configured paths. The match
//!   is by *key name* anywhere in the tree, not by JSON pointer — that's
//!   intentional, so `password` redacts every `password` field regardless
//!   of nesting.

use serde_json::Value;

/// Redact configured headers in-place to `[redacted]`. Header name match is
/// case-insensitive.
pub fn redact_headers(headers: &mut Vec<(String, String)>, names: &[String]) {
    for (k, v) in headers.iter_mut() {
        if names.iter().any(|n| n.eq_ignore_ascii_case(k)) {
            *v = "[redacted]".into();
        }
    }
}

/// Walk a JSON Value recursively, replacing any object key whose name matches
/// (case-insensitively) one of `paths` with the string `[redacted]`.
pub fn redact_body_paths(value: &mut Value, paths: &[String]) {
    match value {
        Value::Object(map) => {
            for (k, v) in map.iter_mut() {
                if paths.iter().any(|p| p.eq_ignore_ascii_case(k)) {
                    *v = Value::String("[redacted]".into());
                } else {
                    redact_body_paths(v, paths);
                }
            }
        }
        Value::Array(arr) => {
            for v in arr.iter_mut() {
                redact_body_paths(v, paths);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn header_match_is_case_insensitive() {
        let mut headers = vec![
            ("Authorization".into(), "Bearer xyz".into()),
            ("X-Trace-Id".into(), "abc123".into()),
        ];
        redact_headers(&mut headers, &["authorization".into()]);
        assert_eq!(headers[0].1, "[redacted]");
        assert_eq!(headers[1].1, "abc123");
    }

    #[test]
    fn header_no_match_left_alone() {
        let mut headers = vec![("Accept".into(), "application/json".into())];
        redact_headers(&mut headers, &["authorization".into()]);
        assert_eq!(headers[0].1, "application/json");
    }

    #[test]
    fn nested_json_key_match() {
        let mut v = json!({
            "user": {
                "name": "alice",
                "credentials": {
                    "password": "hunter2",
                    "token": "t-1"
                }
            },
            "meta": [
                { "Password": "another" },
                { "ok": true }
            ]
        });
        redact_body_paths(&mut v, &["password".into()]);
        assert_eq!(v["user"]["credentials"]["password"], json!("[redacted]"));
        assert_eq!(v["user"]["credentials"]["token"], json!("t-1"));
        assert_eq!(v["meta"][0]["Password"], json!("[redacted]"));
        assert_eq!(v["meta"][1]["ok"], json!(true));
        assert_eq!(v["user"]["name"], json!("alice"));
    }

    #[test]
    fn non_matching_keys_untouched() {
        let original = json!({"a": 1, "b": [1,2,3], "c": {"d": "e"}});
        let mut v = original.clone();
        redact_body_paths(&mut v, &["password".into()]);
        assert_eq!(v, original);
    }
}
