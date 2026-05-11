use crate::error::{GatewayError, PluginError};
use crate::plugins::{BoxBody, Plugin};
use async_trait::async_trait;
use axum::http::{Request, Response, header::HeaderValue};

/// Map directive plugin — derives an output header from an input value
/// based on pattern rules. Caddy equivalent of the `map` directive.
///
/// Config:
/// ```json
/// {
///   "source": "header:X-Region",
///   "destination": "X-Backend-Pool",
///   "mappings": {
///     "us-east-1": "pool-east",
///     "eu-west-1": "pool-eu",
///     "~^ap-.*": "pool-apac"
///   },
///   "default": "pool-default"
/// }
/// ```
///
/// `source` can be:
///   - `header:Name` — read from request header
///   - `path` — request URI path
///   - `method` — HTTP method
///   - `host` — Host header
///   - `query:param` — query parameter value
pub struct MapVarsPlugin {
    source: MapSource,
    destination: String,
    mappings: Vec<MapEntry>,
    default: Option<String>,
}

enum MapSource {
    Header(String),
    Path,
    Method,
    Host,
    Query(String),
}

struct MapEntry {
    pattern: MapPattern,
    value: String,
}

enum MapPattern {
    Exact(String),
    Regex(regex::Regex),
}

impl MapVarsPlugin {
    pub fn from_config(cfg: &serde_json::Value) -> Result<Self, GatewayError> {
        let source_str = cfg
            .get("source")
            .and_then(|v| v.as_str())
            .ok_or_else(|| GatewayError::BadRequest("map: `source` is required".into()))?;

        let source = if let Some(hdr) = source_str.strip_prefix("header:") {
            MapSource::Header(hdr.to_string())
        } else if let Some(param) = source_str.strip_prefix("query:") {
            MapSource::Query(param.to_string())
        } else {
            match source_str {
                "path" => MapSource::Path,
                "method" => MapSource::Method,
                "host" => MapSource::Host,
                other => {
                    return Err(GatewayError::BadRequest(format!(
                        "map: unknown source `{other}`"
                    )));
                }
            }
        };

        let destination = cfg
            .get("destination")
            .and_then(|v| v.as_str())
            .ok_or_else(|| GatewayError::BadRequest("map: `destination` is required".into()))?
            .to_string();

        let mut mappings = Vec::new();
        if let Some(obj) = cfg.get("mappings").and_then(|v| v.as_object()) {
            for (pattern_str, value) in obj {
                let val = value
                    .as_str()
                    .ok_or_else(|| {
                        GatewayError::BadRequest(format!(
                            "map: value for pattern `{pattern_str}` must be a string"
                        ))
                    })?
                    .to_string();

                let pattern = if let Some(regex_str) = pattern_str.strip_prefix('~') {
                    let re = regex::Regex::new(regex_str).map_err(|e| {
                        GatewayError::BadRequest(format!("map: invalid regex `{regex_str}`: {e}"))
                    })?;
                    MapPattern::Regex(re)
                } else {
                    MapPattern::Exact(pattern_str.clone())
                };

                mappings.push(MapEntry { pattern, value: val });
            }
        }

        let default = cfg
            .get("default")
            .and_then(|v| v.as_str())
            .map(String::from);

        Ok(Self {
            source,
            destination,
            mappings,
            default,
        })
    }

    fn resolve_source(&self, req: &Request<axum::body::Body>) -> Option<String> {
        match &self.source {
            MapSource::Header(name) => req
                .headers()
                .get(name.as_str())
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string()),
            MapSource::Path => Some(req.uri().path().to_string()),
            MapSource::Method => Some(req.method().as_str().to_string()),
            MapSource::Host => req
                .headers()
                .get("host")
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string()),
            MapSource::Query(param) => req.uri().query().and_then(|qs| {
                url::form_urlencoded::parse(qs.as_bytes())
                    .find(|(k, _)| k == param)
                    .map(|(_, v)| v.to_string())
            }),
        }
    }

    fn lookup(&self, input: &str) -> Option<&str> {
        for entry in &self.mappings {
            let matches = match &entry.pattern {
                MapPattern::Exact(s) => s == input,
                MapPattern::Regex(re) => re.is_match(input),
            };
            if matches {
                return Some(&entry.value);
            }
        }
        None
    }
}

#[async_trait]
impl Plugin for MapVarsPlugin {
    fn name(&self) -> &str {
        "map"
    }

    async fn on_request(
        &self,
        req: &mut Request<axum::body::Body>,
    ) -> Result<(), PluginError> {
        let input = self.resolve_source(req);
        let output = input
            .as_deref()
            .and_then(|inp| self.lookup(inp))
            .or(self.default.as_deref());

        if let Some(val) = output {
            if let Ok(hv) = HeaderValue::from_str(val) {
                if let Ok(name) = axum::http::HeaderName::from_bytes(self.destination.as_bytes()) {
                    req.headers_mut().insert(name, hv);
                }
            }
        }
        Ok(())
    }

    async fn on_response(&self, _resp: &mut Response<BoxBody>) -> Result<(), PluginError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn exact_match() {
        let cfg = json!({
            "source": "header:X-Region",
            "destination": "X-Pool",
            "mappings": {
                "us-east-1": "pool-east",
                "eu-west-1": "pool-eu"
            },
            "default": "pool-default"
        });
        let plugin = MapVarsPlugin::from_config(&cfg).unwrap();
        assert_eq!(plugin.lookup("us-east-1"), Some("pool-east"));
        assert_eq!(plugin.lookup("eu-west-1"), Some("pool-eu"));
        assert_eq!(plugin.lookup("ap-south-1"), None);
    }

    #[test]
    fn regex_match() {
        let cfg = json!({
            "source": "path",
            "destination": "X-Api-Version",
            "mappings": {
                "~^/api/v1/": "v1",
                "~^/api/v2/": "v2"
            }
        });
        let plugin = MapVarsPlugin::from_config(&cfg).unwrap();
        assert_eq!(plugin.lookup("/api/v1/users"), Some("v1"));
        assert_eq!(plugin.lookup("/api/v2/users"), Some("v2"));
        assert_eq!(plugin.lookup("/api/v3/users"), None);
    }
}
