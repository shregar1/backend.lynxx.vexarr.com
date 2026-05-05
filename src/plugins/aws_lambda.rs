// NOTE on binary size: this plugin pulls in `aws-config` and `aws-sdk-lambda`,
// which together add ~5MB to the release binary because they bring an
// independent SigV4 implementation and the full smithy runtime. Hand-rolling
// SigV4 is risky enough that we accept the cost for v1; a future refactor
// could put this behind a `aws-lambda` cargo feature so deployments that
// don't need it can opt out.

use crate::error::{GatewayError, PluginError};
use crate::plugins::{BoxBody, Plugin};
use async_trait::async_trait;
use aws_sdk_lambda::primitives::Blob;
use aws_sdk_lambda::types::InvocationType;
use axum::http::{Request, Response};
use http_body_util::BodyExt;
use serde_json::json;
use std::sync::Arc;
use tokio::sync::OnceCell;

/// AWS Lambda short-circuit. Replaces the upstream call with a Lambda
/// invocation and returns the function's response payload as the proxy body.
pub struct AwsLambdaPlugin {
    function_name: String,
    region: String,
    invocation_type: InvocationType,
    qualifier: Option<String>,
    aws_access_key_id: Option<String>,
    aws_secret_access_key: Option<String>,
    forward_request_body: bool,
    forward_request_headers: bool,
    client: Arc<OnceCell<aws_sdk_lambda::Client>>,
}

impl AwsLambdaPlugin {
    pub fn from_config(cfg: &serde_json::Value) -> Result<Self, GatewayError> {
        let function_name = cfg
            .get("function_name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| GatewayError::BadRequest("aws-lambda: `function_name` is required".into()))?
            .to_string();
        let region = cfg
            .get("region")
            .and_then(|v| v.as_str())
            .unwrap_or("us-east-1")
            .to_string();
        let invocation_type = match cfg
            .get("invocation_type")
            .and_then(|v| v.as_str())
            .unwrap_or("RequestResponse")
        {
            "Event" => InvocationType::Event,
            "DryRun" => InvocationType::DryRun,
            _ => InvocationType::RequestResponse,
        };
        let qualifier = cfg
            .get("qualifier")
            .and_then(|v| v.as_str())
            .map(String::from);
        let aws_access_key_id = cfg
            .get("aws_access_key_id")
            .and_then(|v| v.as_str())
            .map(String::from);
        let aws_secret_access_key = cfg
            .get("aws_secret_access_key")
            .and_then(|v| v.as_str())
            .map(String::from);
        let forward_request_body = cfg
            .get("forward_request_body")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        let forward_request_headers = cfg
            .get("forward_request_headers")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        Ok(Self {
            function_name,
            region,
            invocation_type,
            qualifier,
            aws_access_key_id,
            aws_secret_access_key,
            forward_request_body,
            forward_request_headers,
            client: Arc::new(OnceCell::new()),
        })
    }

    async fn client(&self) -> &aws_sdk_lambda::Client {
        self.client
            .get_or_init(|| async {
                let region = aws_config::Region::new(self.region.clone());
                let mut loader = aws_config::defaults(aws_config::BehaviorVersion::latest())
                    .region(region);
                if let (Some(id), Some(secret)) =
                    (&self.aws_access_key_id, &self.aws_secret_access_key)
                {
                    let creds = aws_sdk_lambda::config::Credentials::new(
                        id.clone(),
                        secret.clone(),
                        None,
                        None,
                        "lynxx-static",
                    );
                    loader = loader.credentials_provider(creds);
                }
                let cfg = loader.load().await;
                aws_sdk_lambda::Client::new(&cfg)
            })
            .await
    }
}

#[async_trait]
impl Plugin for AwsLambdaPlugin {
    fn name(&self) -> &str {
        "aws-lambda"
    }

    async fn on_request(&self, req: &mut Request<axum::body::Body>) -> Result<(), PluginError> {
        // Build the JSON event in the API Gateway proxy integration shape so
        // existing Lambda handlers can be used unchanged.
        let mut headers = serde_json::Map::new();
        if self.forward_request_headers {
            for (k, v) in req.headers() {
                if let Ok(s) = v.to_str() {
                    headers.insert(k.as_str().to_string(), json!(s));
                }
            }
        }

        let body_str = if self.forward_request_body {
            // Drain the body. After this, the upstream call would receive an
            // empty body — but we short-circuit so it doesn't matter.
            let body = std::mem::replace(req.body_mut(), axum::body::Body::empty());
            let bytes = body
                .collect()
                .await
                .map_err(|e| PluginError::Internal(format!("body collect: {e}")))?
                .to_bytes();
            String::from_utf8_lossy(&bytes).to_string()
        } else {
            String::new()
        };

        let event = json!({
            "httpMethod": req.method().as_str(),
            "path": req.uri().path(),
            "queryStringParameters": req.uri().query(),
            "headers": serde_json::Value::Object(headers),
            "body": body_str,
            "isBase64Encoded": false,
        });
        let payload = serde_json::to_vec(&event)
            .map_err(|e| PluginError::Internal(format!("aws-lambda payload: {e}")))?;

        let client = self.client().await;
        let mut call = client
            .invoke()
            .function_name(&self.function_name)
            .invocation_type(self.invocation_type.clone())
            .payload(Blob::new(payload));
        if let Some(q) = &self.qualifier {
            call = call.qualifier(q);
        }

        let resp = call
            .send()
            .await
            .map_err(|e| PluginError::Internal(format!("aws-lambda invoke: {e}")))?;

        let status = resp.status_code() as u16;
        let body_blob = resp.payload().map(|b| b.as_ref().to_vec()).unwrap_or_default();

        // If the Lambda returned an API-GW-shaped response, surface its
        // statusCode + body verbatim. Otherwise treat the whole payload as
        // the response body with status 200.
        let (status_out, headers_out, body_out) = match serde_json::from_slice::<serde_json::Value>(
            &body_blob,
        ) {
            Ok(v) if v.get("statusCode").is_some() => {
                let s = v
                    .get("statusCode")
                    .and_then(|x| x.as_u64())
                    .unwrap_or(status as u64) as u16;
                let b = v
                    .get("body")
                    .map(|x| match x {
                        serde_json::Value::String(s) => s.clone().into_bytes(),
                        other => other.to_string().into_bytes(),
                    })
                    .unwrap_or_default();
                let mut hdrs = vec![("Content-Type".to_string(), "application/json".to_string())];
                if let Some(obj) = v.get("headers").and_then(|h| h.as_object()) {
                    hdrs.clear();
                    for (k, val) in obj {
                        if let Some(s) = val.as_str() {
                            hdrs.push((k.clone(), s.to_string()));
                        }
                    }
                }
                (s, hdrs, b)
            }
            _ => (
                if status == 0 { 200 } else { status },
                vec![("Content-Type".to_string(), "application/json".to_string())],
                body_blob,
            ),
        };

        Err(PluginError::ShortCircuit {
            status: status_out,
            headers: headers_out,
            body: body_out,
        })
    }

    async fn on_response(&self, _resp: &mut Response<BoxBody>) -> Result<(), PluginError> {
        Ok(())
    }
}
