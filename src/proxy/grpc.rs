use crate::error::GatewayError;
use axum::body::Body;
use axum::http::{HeaderName, HeaderValue, Request, Response, Uri};
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::client::legacy::Client;
use hyper_util::rt::TokioExecutor;

/// HTTP/2 (h2c) client used for gRPC streaming. Built once and shared.
pub type GrpcClient = Client<HttpConnector, Body>;

pub fn build_grpc_client() -> GrpcClient {
    let mut connector = HttpConnector::new();
    connector.enforce_http(false);
    Client::builder(TokioExecutor::new())
        .http2_only(true)
        .build(connector)
}

/// Forwards a gRPC request (HTTP/2 + trailers + streaming) to the upstream
/// without buffering the body. TLS upstreams are out of scope for v1.
pub async fn handle_grpc(
    client: &GrpcClient,
    req: Request<Body>,
    upstream_url: String,
) -> Result<Response<Body>, GatewayError> {
    let parsed = url::Url::parse(&upstream_url)
        .map_err(|e| GatewayError::BadRequest(format!("invalid upstream url: {e}")))?;

    // Accept http:// and h2c:// for clear-text HTTP/2.
    let normalized = match parsed.scheme() {
        "http" | "h2c" => {
            let mut s = upstream_url.clone();
            if parsed.scheme() == "h2c" {
                s.replace_range(0..3, "http");
            }
            s
        }
        "https" => {
            return Err(GatewayError::BadRequest(
                "https grpc upstream not yet supported, use http:// or h2c://".into(),
            ));
        }
        other => {
            return Err(GatewayError::BadRequest(format!(
                "unsupported grpc upstream scheme: {other}"
            )));
        }
    };

    let uri: Uri = normalized
        .parse()
        .map_err(|e| GatewayError::BadRequest(format!("invalid upstream uri: {e}")))?;

    let (mut parts, body) = req.into_parts();
    parts.uri = uri;
    // Strip hop-by-hop headers that don't make sense to forward.
    parts.headers.remove("host");
    parts.headers.remove("connection");

    // Force HTTP/2 — gRPC requires it, and the client is configured h2-only.
    parts.version = axum::http::Version::HTTP_2;

    let upstream_req = Request::from_parts(parts, body);

    let upstream_resp = client
        .request(upstream_req)
        .await
        .map_err(|e| GatewayError::UpstreamError(format!("grpc upstream: {e}")))?;

    let (parts, incoming) = upstream_resp.into_parts();

    // Wrap the upstream Incoming body as an axum Body. Body::new preserves
    // both data frames and trailers so HTTP/2 trailers propagate to the client.
    let mut resp = Response::new(Body::new(incoming));
    *resp.status_mut() = parts.status;
    *resp.version_mut() = parts.version;

    for (name, value) in parts.headers.iter() {
        // axum serves over hyper, so just clone the header map through.
        if let (Ok(n), Ok(v)) = (
            HeaderName::from_bytes(name.as_str().as_bytes()),
            HeaderValue::from_bytes(value.as_bytes()),
        ) {
            resp.headers_mut().insert(n, v);
        }
    }

    Ok(resp)
}
