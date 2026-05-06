//! GCS V4 signed URL generation via the IAM Credentials API.
//!
//! When running on Cloud Run / GCE, the metadata server hands out a
//! bearer token for the attached service account. We call the IAM
//! Credentials `signBlob` endpoint with that token to sign the GCS V4
//! "string-to-sign", then assemble the signed URL. No private key
//! file lives in the binary or on disk.
//!
//! Required IAM:
//! - The SA must have `roles/iam.serviceAccountTokenCreator` on itself
//!   (self-impersonation) so it can sign with its own Google-managed key.
//! - The SA must have `roles/storage.objectViewer` on the bucket (the
//!   signed URL carries the SA's identity; access is checked when the
//!   URL is dereferenced, not when it's signed).

use anyhow::{anyhow, Result};
use chrono::{DateTime, Utc};
use hmac::Hmac;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const METADATA_URL: &str =
    "http://metadata.google.internal/computeMetadata/v1/instance/service-accounts/default/token";
const METADATA_EMAIL_URL: &str =
    "http://metadata.google.internal/computeMetadata/v1/instance/service-accounts/default/email";
const SIGN_BLOB_URL: &str =
    "https://iamcredentials.googleapis.com/v1/projects/-/serviceAccounts/{sa}:signBlob";

#[derive(Clone)]
pub struct GcsSigner {
    bucket: String,
    /// In-process cache for the SA email + bearer token, refreshed
    /// shortly before the metadata-server token expires.
    state: std::sync::Arc<tokio::sync::Mutex<TokenState>>,
}

#[derive(Default)]
struct TokenState {
    email: Option<String>,
    token: Option<String>,
    expires_at: Option<DateTime<Utc>>,
}

#[derive(Deserialize)]
struct MetadataToken {
    access_token: String,
    expires_in: i64,
}

#[derive(Serialize)]
struct SignBlobRequest {
    payload: String,
}

#[derive(Deserialize)]
struct SignBlobResponse {
    #[serde(rename = "signedBlob")]
    signed_blob: String,
}

impl GcsSigner {
    pub fn new(bucket: impl Into<String>) -> Self {
        Self {
            bucket: bucket.into(),
            state: std::sync::Arc::new(tokio::sync::Mutex::new(TokenState::default())),
        }
    }

    pub fn bucket(&self) -> &str {
        &self.bucket
    }

    /// Build a V4 signed URL for `GET <bucket>/<object>` valid for
    /// `valid_for_secs` seconds. The signed URL embeds the SA's email
    /// and the signature; GCS verifies both server-side at request time.
    pub async fn sign_get(&self, object: &str, valid_for_secs: i64) -> Result<String> {
        let (email, token) = self.identity().await?;
        let now = Utc::now();
        let date = now.format("%Y%m%dT%H%M%SZ").to_string();
        let datestamp = now.format("%Y%m%d").to_string();
        let credential_scope = format!("{datestamp}/auto/storage/goog4_request");
        let credential = format!("{email}/{credential_scope}");

        // Object path needs URL-encoding — only the segments, not the
        // slashes between them.
        let encoded_object: String = object
            .split('/')
            .map(|seg| url_encode_path(seg))
            .collect::<Vec<_>>()
            .join("/");
        let canonical_uri = format!("/{}/{}", self.bucket, encoded_object);

        // Query parameters that go INTO the signature, sorted alphabetically.
        let mut q: Vec<(&str, String)> = vec![
            ("X-Goog-Algorithm", "GOOG4-RSA-SHA256".to_string()),
            ("X-Goog-Credential", url_encode_value(&credential)),
            ("X-Goog-Date", date.clone()),
            ("X-Goog-Expires", valid_for_secs.to_string()),
            ("X-Goog-SignedHeaders", "host".to_string()),
        ];
        q.sort_by(|a, b| a.0.cmp(b.0));
        let canonical_query: String = q
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join("&");

        // Canonical request — host header is the only signed header.
        let canonical_headers = format!("host:storage.googleapis.com\n");
        let signed_headers = "host";
        let payload_hash = "UNSIGNED-PAYLOAD";
        let canonical_request = format!(
            "GET\n{canonical_uri}\n{canonical_query}\n{canonical_headers}\n{signed_headers}\n{payload_hash}"
        );

        // String-to-sign — sha256 the canonical request and wrap with the V4 prefix.
        let cr_hash = sha256_hex(canonical_request.as_bytes());
        let string_to_sign = format!("GOOG4-RSA-SHA256\n{date}\n{credential_scope}\n{cr_hash}");

        // Sign via IAM Credentials signBlob (returns base64).
        let sig_b64 = self
            .sign_blob(&email, &token, string_to_sign.as_bytes())
            .await?;
        let sig_bytes = base64::engine::general_purpose::STANDARD.decode(&sig_b64)?;
        let sig_hex = bytes_to_hex(&sig_bytes);

        Ok(format!(
            "https://storage.googleapis.com{canonical_uri}?{canonical_query}&X-Goog-Signature={sig_hex}"
        ))
    }

    async fn identity(&self) -> Result<(String, String)> {
        let mut state = self.state.lock().await;
        let now = Utc::now();
        // Refresh if missing or within 60s of expiry.
        let needs_refresh = match state.expires_at {
            Some(exp) => exp - chrono::Duration::seconds(60) <= now,
            None => true,
        };
        if needs_refresh {
            let client = reqwest::Client::new();

            // SA email — only fetch once per process; it doesn't change.
            if state.email.is_none() {
                let email = client
                    .get(METADATA_EMAIL_URL)
                    .header("Metadata-Flavor", "Google")
                    .send()
                    .await?
                    .error_for_status()?
                    .text()
                    .await?
                    .trim()
                    .to_string();
                state.email = Some(email);
            }

            // Access token.
            let resp: MetadataToken = client
                .get(METADATA_URL)
                .header("Metadata-Flavor", "Google")
                .send()
                .await?
                .error_for_status()?
                .json()
                .await?;
            state.expires_at = Some(now + chrono::Duration::seconds(resp.expires_in));
            state.token = Some(resp.access_token);
        }
        Ok((state.email.clone().unwrap(), state.token.clone().unwrap()))
    }

    async fn sign_blob(&self, email: &str, token: &str, payload: &[u8]) -> Result<String> {
        let url = SIGN_BLOB_URL.replace("{sa}", email);
        let body = SignBlobRequest {
            payload: base64::engine::general_purpose::STANDARD.encode(payload),
        };
        let client = reqwest::Client::new();
        let resp = client
            .post(&url)
            .bearer_auth(token)
            .json(&body)
            .send()
            .await?;
        let status = resp.status();
        let text = resp.text().await?;
        if !status.is_success() {
            return Err(anyhow!("signBlob {status}: {text}"));
        }
        let parsed: SignBlobResponse = serde_json::from_str(&text)
            .map_err(|e| anyhow!("signBlob response parse: {e} body={text}"))?;
        Ok(parsed.signed_blob)
    }
}

// --- helpers ---------------------------------------------------------------

use base64::Engine;

fn sha256_hex(input: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(input);
    bytes_to_hex(h.finalize().as_slice())
}

fn bytes_to_hex(b: &[u8]) -> String {
    let mut s = String::with_capacity(b.len() * 2);
    for byte in b {
        s.push_str(&format!("{byte:02x}"));
    }
    s
}

/// Percent-encode a path segment per RFC 3986. Letters / digits /
/// `-_.~` stay verbatim; everything else becomes `%HH`.
fn url_encode_path(segment: &str) -> String {
    let mut out = String::with_capacity(segment.len());
    for b in segment.as_bytes() {
        let c = *b;
        if c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_' | b'.' | b'~') {
            out.push(c as char);
        } else {
            out.push_str(&format!("%{c:02X}"));
        }
    }
    out
}

/// Percent-encode a value going into a query-string position. Space → %20.
fn url_encode_value(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for b in value.as_bytes() {
        let c = *b;
        if c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_' | b'.' | b'~') {
            out.push(c as char);
        } else {
            out.push_str(&format!("%{c:02X}"));
        }
    }
    out
}

// HmacSha256 isn't used yet but kept here for future signed cookies / etc.
#[allow(dead_code)]
type HmacSha256 = Hmac<Sha256>;
