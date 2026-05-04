use crate::error::{GatewayError, PluginError};
use crate::plugins::{BoxBody, Plugin};
use async_trait::async_trait;
use axum::http::{Request, Response};
use ipnet::IpNet;
use std::net::IpAddr;
use std::str::FromStr;

pub struct IpRestrictionPlugin {
    allow: Vec<IpNet>,
    deny: Vec<IpNet>,
    has_allow: bool,
}

impl IpRestrictionPlugin {
    pub fn from_config(cfg: &serde_json::Value) -> Result<Self, GatewayError> {
        let allow = parse_cidr_list(cfg.get("allow"))?;
        let deny = parse_cidr_list(cfg.get("deny"))?;
        let has_allow = cfg
            .get("allow")
            .and_then(|v| v.as_array())
            .map(|a| !a.is_empty())
            .unwrap_or(false);
        Ok(Self {
            allow,
            deny,
            has_allow,
        })
    }

    fn extract_client_ip(req: &Request<axum::body::Body>) -> IpAddr {
        let from_header = req
            .headers()
            .get("x-forwarded-for")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.split(',').next())
            .map(|s| s.trim())
            .or_else(|| {
                req.headers()
                    .get("x-real-ip")
                    .and_then(|v| v.to_str().ok())
                    .map(|s| s.trim())
            });

        from_header
            .and_then(|s| IpAddr::from_str(s).ok())
            .unwrap_or_else(|| IpAddr::from_str("0.0.0.0").unwrap())
    }
}

fn parse_cidr_list(val: Option<&serde_json::Value>) -> Result<Vec<IpNet>, GatewayError> {
    let Some(arr) = val.and_then(|v| v.as_array()) else {
        return Ok(Vec::new());
    };
    let mut out = Vec::with_capacity(arr.len());
    for v in arr {
        let s = v.as_str().ok_or_else(|| {
            GatewayError::BadRequest("ip_restriction: cidr must be string".into())
        })?;
        // Allow bare IP without prefix.
        let net = if s.contains('/') {
            IpNet::from_str(s)
                .map_err(|e| GatewayError::BadRequest(format!("invalid cidr {s}: {e}")))?
        } else {
            let ip = IpAddr::from_str(s)
                .map_err(|e| GatewayError::BadRequest(format!("invalid ip {s}: {e}")))?;
            IpNet::from(ip)
        };
        out.push(net);
    }
    Ok(out)
}

#[async_trait]
impl Plugin for IpRestrictionPlugin {
    fn name(&self) -> &str {
        "ip-restriction"
    }

    async fn on_request(&self, req: &mut Request<axum::body::Body>) -> Result<(), PluginError> {
        let ip = Self::extract_client_ip(req);

        if self.deny.iter().any(|net| net.contains(&ip)) {
            return Err(PluginError::Forbidden(format!("ip {ip} denied")));
        }

        if self.has_allow && !self.allow.iter().any(|net| net.contains(&ip)) {
            return Err(PluginError::Forbidden(format!("ip {ip} not allowed")));
        }

        Ok(())
    }

    async fn on_response(&self, _resp: &mut Response<BoxBody>) -> Result<(), PluginError> {
        Ok(())
    }
}
