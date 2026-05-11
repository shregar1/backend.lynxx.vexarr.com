use crate::error::GatewayError;
use async_trait::async_trait;
use std::sync::Arc;

const TXT_TTL: u32 = 120;
const PROPAGATION_POLL_SECS: u64 = 5;
const PROPAGATION_TIMEOUT_SECS: u64 = 120;

#[async_trait]
pub trait DnsProvider: Send + Sync + std::fmt::Debug {
    async fn set_txt_record(
        &self,
        fqdn: &str,
        value: &str,
    ) -> Result<String, GatewayError>;

    async fn clear_txt_record(&self, record_id: &str) -> Result<(), GatewayError>;

    fn provider_name(&self) -> &'static str;
}

// ── Cloudflare ──────────────────────────────────────────────────────

#[derive(Debug)]
pub struct CloudflareDns {
    api_token: String,
    zone_id: String,
    client: reqwest::Client,
}

impl CloudflareDns {
    pub fn new(api_token: String, zone_id: String) -> Self {
        Self {
            api_token,
            zone_id,
            client: reqwest::Client::new(),
        }
    }

    fn api_base(&self) -> String {
        format!(
            "https://api.cloudflare.com/client/v4/zones/{}/dns_records",
            self.zone_id
        )
    }
}

#[async_trait]
impl DnsProvider for CloudflareDns {
    async fn set_txt_record(
        &self,
        fqdn: &str,
        value: &str,
    ) -> Result<String, GatewayError> {
        let body = serde_json::json!({
            "type": "TXT",
            "name": fqdn,
            "content": value,
            "ttl": TXT_TTL,
        });

        let resp = self
            .client
            .post(&self.api_base())
            .bearer_auth(&self.api_token)
            .json(&body)
            .send()
            .await
            .map_err(|e| GatewayError::Internal(format!("cloudflare create TXT: {e}")))?;

        let status = resp.status();
        let text = resp
            .text()
            .await
            .map_err(|e| GatewayError::Internal(format!("cloudflare response body: {e}")))?;

        if !status.is_success() {
            return Err(GatewayError::Internal(format!(
                "cloudflare create TXT failed ({status}): {text}"
            )));
        }

        let json: serde_json::Value = serde_json::from_str(&text)
            .map_err(|e| GatewayError::Internal(format!("cloudflare parse response: {e}")))?;

        let record_id = json["result"]["id"]
            .as_str()
            .ok_or_else(|| GatewayError::Internal("cloudflare: no record id in response".into()))?
            .to_string();

        tracing::info!(fqdn = %fqdn, record_id = %record_id, "cloudflare TXT record created");
        Ok(record_id)
    }

    async fn clear_txt_record(&self, record_id: &str) -> Result<(), GatewayError> {
        let url = format!("{}/{record_id}", self.api_base());
        let resp = self
            .client
            .delete(&url)
            .bearer_auth(&self.api_token)
            .send()
            .await
            .map_err(|e| GatewayError::Internal(format!("cloudflare delete TXT: {e}")))?;

        if !resp.status().is_success() {
            let text = resp.text().await.unwrap_or_default();
            tracing::warn!(record_id = %record_id, "cloudflare delete TXT failed: {text}");
        } else {
            tracing::info!(record_id = %record_id, "cloudflare TXT record deleted");
        }
        Ok(())
    }

    fn provider_name(&self) -> &'static str {
        "cloudflare"
    }
}

// ── Route53 ─────────────────────────────────────────────────────────

#[derive(Debug)]
pub struct Route53Dns {
    hosted_zone_id: String,
    region: String,
    access_key_id: String,
    secret_access_key: String,
    client: reqwest::Client,
}

impl Route53Dns {
    pub fn new(
        hosted_zone_id: String,
        region: String,
        access_key_id: String,
        secret_access_key: String,
    ) -> Self {
        Self {
            hosted_zone_id,
            region,
            access_key_id,
            secret_access_key,
            client: reqwest::Client::new(),
        }
    }

    fn change_batch_xml(&self, action: &str, fqdn: &str, value: &str) -> String {
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<ChangeResourceRecordSetsRequest xmlns="https://route53.amazonaws.com/doc/2013-04-01/">
  <ChangeBatch>
    <Changes>
      <Change>
        <Action>{action}</Action>
        <ResourceRecordSet>
          <Name>{fqdn}</Name>
          <Type>TXT</Type>
          <TTL>{TTL_TTL}</TTL>
          <ResourceRecords>
            <ResourceRecord>
              <Value>"{value}"</Value>
            </ResourceRecord>
          </ResourceRecords>
        </ResourceRecordSet>
      </Change>
    </Changes>
  </ChangeBatch>
</ChangeResourceRecordSetsRequest>"#,
            action = action,
            fqdn = fqdn,
            TTL_TTL = TXT_TTL,
            value = value,
        )
    }

    async fn submit_change(
        &self,
        action: &str,
        fqdn: &str,
        value: &str,
    ) -> Result<(), GatewayError> {
        let url = format!(
            "https://route53.amazonaws.com/2013-04-01/hostedzone/{}/rrset",
            self.hosted_zone_id
        );
        let body = self.change_batch_xml(action, fqdn, value);

        let now = chrono::Utc::now();
        let date_str = now.format("%Y%m%dT%H%M%SZ").to_string();
        let date_short = now.format("%Y%m%d").to_string();

        let content_hash = hex_sha256(body.as_bytes());

        let canonical = format!(
            "POST\n/{}/rrset\n\ncontent-type:application/xml\nhost:route53.amazonaws.com\nx-amz-date:{}\n\ncontent-type;host;x-amz-date\n{}",
            format!("2013-04-01/hostedzone/{}", self.hosted_zone_id),
            date_str,
            content_hash
        );
        let scope = format!("{}/{}/route53/aws4_request", date_short, self.region);
        let string_to_sign = format!(
            "AWS4-HMAC-SHA256\n{}\n{}\n{}",
            date_str,
            scope,
            hex_sha256(canonical.as_bytes())
        );

        let k_date = hmac_sha256(
            format!("AWS4{}", self.secret_access_key).as_bytes(),
            date_short.as_bytes(),
        );
        let k_region = hmac_sha256(&k_date, self.region.as_bytes());
        let k_service = hmac_sha256(&k_region, b"route53");
        let k_signing = hmac_sha256(&k_service, b"aws4_request");
        let signature = hex::encode(hmac_sha256(&k_signing, string_to_sign.as_bytes()));

        let auth_header = format!(
            "AWS4-HMAC-SHA256 Credential={}/{}, SignedHeaders=content-type;host;x-amz-date, Signature={}",
            self.access_key_id, scope, signature
        );

        let resp = self
            .client
            .post(&url)
            .header("Content-Type", "application/xml")
            .header("X-Amz-Date", &date_str)
            .header("Authorization", &auth_header)
            .body(body)
            .send()
            .await
            .map_err(|e| GatewayError::Internal(format!("route53 {action}: {e}")))?;

        if !resp.status().is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(GatewayError::Internal(format!(
                "route53 {action} failed: {text}"
            )));
        }
        Ok(())
    }
}

#[async_trait]
impl DnsProvider for Route53Dns {
    async fn set_txt_record(
        &self,
        fqdn: &str,
        value: &str,
    ) -> Result<String, GatewayError> {
        self.submit_change("UPSERT", fqdn, value).await?;
        tracing::info!(fqdn = %fqdn, "route53 TXT record upserted");
        Ok(format!("{fqdn}:{value}"))
    }

    async fn clear_txt_record(&self, record_id: &str) -> Result<(), GatewayError> {
        let parts: Vec<&str> = record_id.splitn(2, ':').collect();
        if parts.len() != 2 {
            tracing::warn!(record_id = %record_id, "route53: invalid record_id format for cleanup");
            return Ok(());
        }
        if let Err(e) = self.submit_change("DELETE", parts[0], parts[1]).await {
            tracing::warn!(error = %e, "route53 TXT cleanup failed (may already be deleted)");
        } else {
            tracing::info!(fqdn = %parts[0], "route53 TXT record deleted");
        }
        Ok(())
    }

    fn provider_name(&self) -> &'static str {
        "route53"
    }
}

// ── Helpers ─────────────────────────────────────────────────────────

fn hex_sha256(data: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(data))
}

fn hmac_sha256(key: &[u8], data: &[u8]) -> Vec<u8> {
    use hmac::{Hmac, Mac};
    use sha2::Sha256;
    let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("hmac key");
    mac.update(data);
    mac.finalize().into_bytes().to_vec()
}

pub fn build_acme_dns_name(domain: &str) -> String {
    format!("_acme-challenge.{domain}")
}

pub async fn wait_for_propagation(
    fqdn: &str,
    expected: &str,
) -> Result<(), GatewayError> {
    let resolver = hickory_resolver::TokioAsyncResolver::tokio(
        hickory_resolver::config::ResolverConfig::default(),
        hickory_resolver::config::ResolverOpts::default(),
    );

    let deadline =
        tokio::time::Instant::now() + std::time::Duration::from_secs(PROPAGATION_TIMEOUT_SECS);

    loop {
        if tokio::time::Instant::now() >= deadline {
            return Err(GatewayError::Internal(format!(
                "DNS propagation timeout for {fqdn} (waited {PROPAGATION_TIMEOUT_SECS}s)"
            )));
        }

        match resolver.txt_lookup(fqdn).await {
            Ok(lookup) => {
                for record in lookup.iter() {
                    let txt = record.to_string();
                    if txt.trim_matches('"') == expected {
                        tracing::info!(fqdn = %fqdn, "DNS TXT record propagated");
                        return Ok(());
                    }
                }
            }
            Err(_) => {}
        }

        tokio::time::sleep(std::time::Duration::from_secs(PROPAGATION_POLL_SECS)).await;
    }
}

// ── DigitalOcean ───────────────────────────────────────────────────

#[derive(Debug)]
pub struct DigitalOceanDns {
    token: String,
    client: reqwest::Client,
}

impl DigitalOceanDns {
    pub fn new(token: String) -> Self {
        Self {
            token,
            client: reqwest::Client::new(),
        }
    }

    fn extract_domain(fqdn: &str) -> (&str, &str) {
        // Split "sub.example.com" into ("example.com", "sub").
        let parts: Vec<&str> = fqdn.rsplitn(3, '.').collect();
        if parts.len() >= 2 {
            let domain = &fqdn[fqdn.len() - parts[0].len() - parts[1].len() - 1..];
            let subdomain = &fqdn[..fqdn.len() - domain.len() - 1];
            (domain, subdomain)
        } else {
            (fqdn, "@")
        }
    }
}

#[async_trait]
impl DnsProvider for DigitalOceanDns {
    async fn set_txt_record(
        &self,
        fqdn: &str,
        value: &str,
    ) -> Result<String, GatewayError> {
        let acme_fqdn = build_acme_dns_name(fqdn);
        let (domain, name) = Self::extract_domain(&acme_fqdn);

        let resp = self
            .client
            .post(format!(
                "https://api.digitalocean.com/v2/domains/{domain}/records"
            ))
            .bearer_auth(&self.token)
            .json(&serde_json::json!({
                "type": "TXT",
                "name": name,
                "data": value,
                "ttl": TXT_TTL,
            }))
            .send()
            .await
            .map_err(|e| GatewayError::Internal(format!("digitalocean api: {e}")))?;

        if !resp.status().is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(GatewayError::Internal(format!(
                "digitalocean dns create failed: {body}"
            )));
        }

        let json: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| GatewayError::Internal(format!("digitalocean parse: {e}")))?;
        let id = json["domain_record"]["id"]
            .as_u64()
            .map(|n| format!("{domain}:{n}"))
            .unwrap_or_default();

        let _ = wait_for_propagation(&acme_fqdn, value).await;
        Ok(id)
    }

    async fn clear_txt_record(&self, record_id: &str) -> Result<(), GatewayError> {
        if let Some((domain, id)) = record_id.split_once(':') {
            let _ = self
                .client
                .delete(format!(
                    "https://api.digitalocean.com/v2/domains/{domain}/records/{id}"
                ))
                .bearer_auth(&self.token)
                .send()
                .await;
        }
        Ok(())
    }

    fn provider_name(&self) -> &'static str {
        "digitalocean"
    }
}

// ── Google Cloud DNS ───────────────────────────────────────────────

#[derive(Debug)]
pub struct GoogleCloudDns {
    token: String,
    managed_zone: String,
    client: reqwest::Client,
}

impl GoogleCloudDns {
    pub fn new(token: String, managed_zone: String) -> Self {
        Self {
            token,
            managed_zone,
            client: reqwest::Client::new(),
        }
    }
}

#[async_trait]
impl DnsProvider for GoogleCloudDns {
    async fn set_txt_record(
        &self,
        fqdn: &str,
        value: &str,
    ) -> Result<String, GatewayError> {
        let acme_fqdn = format!("{}.", build_acme_dns_name(fqdn));
        let project = &self.managed_zone;

        let resp = self
            .client
            .post(format!(
                "https://dns.googleapis.com/dns/v1/projects/{project}/managedZones/{}/changes",
                project
            ))
            .bearer_auth(&self.token)
            .json(&serde_json::json!({
                "additions": [{
                    "name": acme_fqdn,
                    "type": "TXT",
                    "ttl": TXT_TTL,
                    "rrdatas": [format!("\"{}\"", value)]
                }]
            }))
            .send()
            .await
            .map_err(|e| GatewayError::Internal(format!("gcloud dns api: {e}")))?;

        if !resp.status().is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(GatewayError::Internal(format!(
                "gcloud dns create failed: {body}"
            )));
        }

        let _ = wait_for_propagation(acme_fqdn.trim_end_matches('.'), value).await;
        Ok(acme_fqdn)
    }

    async fn clear_txt_record(&self, record_id: &str) -> Result<(), GatewayError> {
        // Google Cloud DNS requires a deletion change; simplified here.
        tracing::debug!(record = %record_id, "gcloud dns cleanup (manual)");
        Ok(())
    }

    fn provider_name(&self) -> &'static str {
        "gcloud"
    }
}

pub fn from_config(
    cfg: &crate::config::Dns01Config,
) -> Result<Arc<dyn DnsProvider>, GatewayError> {
    match cfg.provider.as_str() {
        "cloudflare" => {
            if cfg.api_token.is_empty() || cfg.zone_id.is_empty() {
                return Err(GatewayError::ConfigError(
                    "dns-01 cloudflare requires api_token and zone_id".into(),
                ));
            }
            Ok(Arc::new(CloudflareDns::new(
                cfg.api_token.clone(),
                cfg.zone_id.clone(),
            )))
        }
        "route53" => {
            if cfg.hosted_zone_id.is_empty() {
                return Err(GatewayError::ConfigError(
                    "dns-01 route53 requires hosted_zone_id".into(),
                ));
            }
            let access_key = if cfg.access_key_id.is_empty() {
                std::env::var("AWS_ACCESS_KEY_ID").unwrap_or_default()
            } else {
                cfg.access_key_id.clone()
            };
            let secret_key = if cfg.secret_access_key.is_empty() {
                std::env::var("AWS_SECRET_ACCESS_KEY").unwrap_or_default()
            } else {
                cfg.secret_access_key.clone()
            };
            let region = if cfg.region.is_empty() {
                std::env::var("AWS_REGION").unwrap_or_else(|_| "us-east-1".into())
            } else {
                cfg.region.clone()
            };
            Ok(Arc::new(Route53Dns::new(
                cfg.hosted_zone_id.clone(),
                region,
                access_key,
                secret_key,
            )))
        }
        "digitalocean" => {
            let token = if cfg.api_token.is_empty() {
                std::env::var("DIGITALOCEAN_TOKEN").unwrap_or_default()
            } else {
                cfg.api_token.clone()
            };
            if token.is_empty() {
                return Err(GatewayError::ConfigError(
                    "dns-01 digitalocean requires api_token or DIGITALOCEAN_TOKEN env".into(),
                ));
            }
            Ok(Arc::new(DigitalOceanDns::new(token)))
        }
        "gcloud" | "google" | "google-cloud" => {
            let token = if cfg.api_token.is_empty() {
                std::env::var("GCLOUD_DNS_TOKEN").unwrap_or_default()
            } else {
                cfg.api_token.clone()
            };
            let project = if cfg.zone_id.is_empty() {
                std::env::var("GCLOUD_PROJECT").unwrap_or_default()
            } else {
                // Reuse zone_id field for the managed zone name.
                cfg.zone_id.clone()
            };
            if token.is_empty() || project.is_empty() {
                return Err(GatewayError::ConfigError(
                    "dns-01 gcloud requires api_token and zone_id (managed zone)".into(),
                ));
            }
            Ok(Arc::new(GoogleCloudDns::new(token, project)))
        }
        other => Err(GatewayError::ConfigError(format!(
            "unknown dns-01 provider: {other} (supported: cloudflare, route53, digitalocean, gcloud)"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn acme_dns_name() {
        assert_eq!(
            build_acme_dns_name("example.com"),
            "_acme-challenge.example.com"
        );
    }

    #[test]
    fn cloudflare_provider_name() {
        let p = CloudflareDns::new("tok".into(), "zone".into());
        assert_eq!(p.provider_name(), "cloudflare");
    }

    #[test]
    fn route53_provider_name() {
        let p = Route53Dns::new("zone".into(), "us-east-1".into(), "ak".into(), "sk".into());
        assert_eq!(p.provider_name(), "route53");
    }

    #[test]
    fn route53_change_batch_xml() {
        let p = Route53Dns::new("Z123".into(), "us-east-1".into(), "ak".into(), "sk".into());
        let xml = p.change_batch_xml("UPSERT", "_acme-challenge.example.com", "test-value");
        assert!(xml.contains("UPSERT"));
        assert!(xml.contains("_acme-challenge.example.com"));
        assert!(xml.contains("\"test-value\""));
        assert!(xml.contains("TXT"));
    }

    #[test]
    fn hex_sha256_known_vector() {
        let result = hex_sha256(b"");
        assert_eq!(
            result,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }
}
