use anyhow::{anyhow, Context, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::crds::{LynxxPluginSpec, LynxxRouteSpec, LynxxServiceSpec, LynxxTarget};

#[derive(Clone)]
pub struct GatewayClient {
    base: String,
    http: Client,
}

#[derive(Debug, Deserialize)]
struct ServiceResp {
    pub id: Uuid,
    pub name: String,
}

#[derive(Debug, Deserialize)]
struct RouteResp {
    pub id: Uuid,
    pub paths: Vec<String>,
    pub service_id: Uuid,
}

#[derive(Debug, Deserialize)]
struct PluginResp {
    pub id: Uuid,
    pub name: String,
    #[serde(default)]
    pub service_id: Option<Uuid>,
    #[serde(default)]
    pub route_id: Option<Uuid>,
}

#[derive(Debug, Deserialize)]
struct TargetResp {
    pub id: Uuid,
    pub url: String,
}

#[derive(Serialize)]
struct CreateServiceBody<'a> {
    name: &'a str,
    url: &'a str,
    retries: u32,
    connect_timeout_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    lb_strategy: Option<String>,
}

#[derive(Serialize)]
struct CreateRouteBody<'a> {
    paths: &'a [String],
    methods: &'a [String],
    service_id: Uuid,
    strip_path: bool,
}

#[derive(Serialize)]
struct CreatePluginBody<'a> {
    name: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    service_id: Option<Uuid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    route_id: Option<Uuid>,
    enabled: bool,
    config: &'a Value,
}

#[derive(Serialize)]
struct CreateTargetBody<'a> {
    url: &'a str,
    weight: u32,
}

impl GatewayClient {
    pub fn new(base: String) -> Self {
        let base = base.trim_end_matches('/').to_string();
        Self {
            base,
            http: Client::new(),
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base, path)
    }

    // --- services ---

    async fn find_service_by_name(&self, name: &str) -> Result<Option<ServiceResp>> {
        let resp: Vec<ServiceResp> = self
            .http
            .get(self.url("/services"))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        Ok(resp.into_iter().find(|s| s.name == name))
    }

    pub async fn upsert_service(&self, name: &str, spec: &LynxxServiceSpec) -> Result<Uuid> {
        let body = CreateServiceBody {
            name,
            url: &spec.url,
            retries: spec.retries,
            connect_timeout_ms: spec.connect_timeout_ms,
            lb_strategy: spec.lb_strategy.clone(),
        };
        if let Some(existing) = self.find_service_by_name(name).await? {
            let resp = self
                .http
                .put(self.url(&format!("/services/{}", existing.id)))
                .json(&body)
                .send()
                .await?;
            if !resp.status().is_success() {
                let status = resp.status();
                let text = resp.text().await.unwrap_or_default();
                return Err(anyhow!("update service failed {status}: {text}"));
            }
            Ok(existing.id)
        } else {
            let resp: ServiceResp = self
                .http
                .post(self.url("/services"))
                .json(&body)
                .send()
                .await?
                .error_for_status()
                .context("create service")?
                .json()
                .await?;
            Ok(resp.id)
        }
    }

    pub async fn delete_service(&self, id: Uuid) -> Result<()> {
        let resp = self
            .http
            .delete(self.url(&format!("/services/{}", id)))
            .send()
            .await?;
        if resp.status().is_success() || resp.status() == reqwest::StatusCode::NOT_FOUND {
            Ok(())
        } else {
            Err(anyhow!("delete service failed: {}", resp.status()))
        }
    }

    // --- targets ---

    pub async fn upsert_targets(&self, service_id: Uuid, targets: &[LynxxTarget]) -> Result<()> {
        // Reconcile: list existing, delete those not in spec, create missing.
        let existing: Vec<TargetResp> = self
            .http
            .get(self.url(&format!("/services/{}/targets", service_id)))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;

        let desired_urls: std::collections::HashSet<&str> =
            targets.iter().map(|t| t.url.as_str()).collect();
        for ex in &existing {
            if !desired_urls.contains(ex.url.as_str()) {
                let _ = self
                    .http
                    .delete(self.url(&format!("/targets/{}", ex.id)))
                    .send()
                    .await?;
            }
        }
        let existing_urls: std::collections::HashSet<&str> =
            existing.iter().map(|t| t.url.as_str()).collect();
        for t in targets {
            if !existing_urls.contains(t.url.as_str()) {
                let body = CreateTargetBody {
                    url: &t.url,
                    weight: t.weight.max(1),
                };
                let resp = self
                    .http
                    .post(self.url(&format!("/services/{}/targets", service_id)))
                    .json(&body)
                    .send()
                    .await?;
                if !resp.status().is_success() {
                    let s = resp.status();
                    let txt = resp.text().await.unwrap_or_default();
                    return Err(anyhow!("create target failed {s}: {txt}"));
                }
            }
        }
        Ok(())
    }

    // --- routes ---

    async fn find_route_for_service(
        &self,
        service_id: Uuid,
        paths: &[String],
    ) -> Result<Option<RouteResp>> {
        let all: Vec<RouteResp> = self
            .http
            .get(self.url("/routes"))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        Ok(all.into_iter().find(|r| {
            r.service_id == service_id
                && r.paths.len() == paths.len()
                && r.paths.iter().all(|p| paths.contains(p))
        }))
    }

    pub async fn upsert_route(
        &self,
        existing_id: Option<Uuid>,
        service_id: Uuid,
        spec: &LynxxRouteSpec,
    ) -> Result<Uuid> {
        let body = CreateRouteBody {
            paths: &spec.paths,
            methods: &spec.methods,
            service_id,
            strip_path: spec.strip_path,
        };
        if let Some(id) = existing_id {
            let resp = self
                .http
                .put(self.url(&format!("/routes/{}", id)))
                .json(&body)
                .send()
                .await?;
            if resp.status().is_success() {
                return Ok(id);
            }
            // fallthrough to create if route is gone
        }
        // try by signature
        if let Some(found) = self.find_route_for_service(service_id, &spec.paths).await? {
            let resp = self
                .http
                .put(self.url(&format!("/routes/{}", found.id)))
                .json(&body)
                .send()
                .await?
                .error_for_status()?;
            let _ = resp;
            return Ok(found.id);
        }
        let resp: RouteResp = self
            .http
            .post(self.url("/routes"))
            .json(&body)
            .send()
            .await?
            .error_for_status()
            .context("create route")?
            .json()
            .await?;
        Ok(resp.id)
    }

    pub async fn delete_route(&self, id: Uuid) -> Result<()> {
        let resp = self
            .http
            .delete(self.url(&format!("/routes/{}", id)))
            .send()
            .await?;
        if resp.status().is_success() || resp.status() == reqwest::StatusCode::NOT_FOUND {
            Ok(())
        } else {
            Err(anyhow!("delete route failed: {}", resp.status()))
        }
    }

    // --- plugins ---

    async fn find_plugin_by_signature(
        &self,
        name: &str,
        service_id: Option<Uuid>,
        route_id: Option<Uuid>,
    ) -> Result<Option<PluginResp>> {
        let all: Vec<PluginResp> = self
            .http
            .get(self.url("/plugins"))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        Ok(all
            .into_iter()
            .find(|p| p.name == name && p.service_id == service_id && p.route_id == route_id))
    }

    pub async fn upsert_plugin(
        &self,
        existing_id: Option<Uuid>,
        spec: &LynxxPluginSpec,
        service_id: Option<Uuid>,
        route_id: Option<Uuid>,
    ) -> Result<Uuid> {
        let cfg = if spec.config.is_null() {
            json!({})
        } else {
            spec.config.clone()
        };
        let body = CreatePluginBody {
            name: &spec.name,
            service_id,
            route_id,
            enabled: spec.enabled,
            config: &cfg,
        };
        if let Some(id) = existing_id {
            let resp = self
                .http
                .put(self.url(&format!("/plugins/{}", id)))
                .json(&body)
                .send()
                .await?;
            if resp.status().is_success() {
                return Ok(id);
            }
        }
        if let Some(found) = self
            .find_plugin_by_signature(&spec.name, service_id, route_id)
            .await?
        {
            let resp = self
                .http
                .put(self.url(&format!("/plugins/{}", found.id)))
                .json(&body)
                .send()
                .await?;
            if !resp.status().is_success() {
                let s = resp.status();
                let t = resp.text().await.unwrap_or_default();
                return Err(anyhow!("update plugin failed {s}: {t}"));
            }
            return Ok(found.id);
        }
        let resp: PluginResp = self
            .http
            .post(self.url("/plugins"))
            .json(&body)
            .send()
            .await?
            .error_for_status()
            .context("create plugin")?
            .json()
            .await?;
        Ok(resp.id)
    }

    pub async fn delete_plugin(&self, id: Uuid) -> Result<()> {
        let resp = self
            .http
            .delete(self.url(&format!("/plugins/{}", id)))
            .send()
            .await?;
        if resp.status().is_success() || resp.status() == reqwest::StatusCode::NOT_FOUND {
            Ok(())
        } else {
            Err(anyhow!("delete plugin failed: {}", resp.status()))
        }
    }
}
