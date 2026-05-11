use crate::error::{GatewayError, PluginError};
use crate::plugins::{BoxBody, Plugin};
use async_trait::async_trait;
use axum::body::Body;
use axum::http::header::{ACCEPT_ENCODING, CONTENT_ENCODING, CONTENT_LENGTH, CONTENT_TYPE, VARY};
use axum::http::{HeaderValue, Request, Response};
use http_body_util::BodyExt;
use std::io::Write;

#[derive(Clone, Copy, Debug, PartialEq)]
enum CompressionAlgo {
    Gzip,
    Zstd,
    Brotli,
}

#[derive(Clone, Debug)]
struct NegotiatedEncoding(CompressionAlgo);

const DEFAULT_MIN_BODY_SIZE: usize = 256;

const DEFAULT_CONTENT_TYPES: &[&str] = &[
    "text/",
    "application/json",
    "application/xml",
    "application/javascript",
    "application/x-javascript",
    "application/xhtml+xml",
    "image/svg+xml",
];

pub struct ResponseCompressionPlugin {
    algorithms: Vec<CompressionAlgo>,
    min_body_size: usize,
    content_types: Vec<String>,
    gzip_level: u32,
    zstd_level: i32,
    brotli_level: u32,
}

impl ResponseCompressionPlugin {
    pub fn from_config(cfg: &serde_json::Value) -> Result<Self, GatewayError> {
        let algorithms = if let Some(arr) = cfg.get("algorithms").and_then(|v| v.as_array()) {
            arr.iter()
                .filter_map(|v| v.as_str())
                .filter_map(|s| match s {
                    "gzip" | "gz" => Some(CompressionAlgo::Gzip),
                    "zstd" | "zstandard" => Some(CompressionAlgo::Zstd),
                    "br" | "brotli" => Some(CompressionAlgo::Brotli),
                    _ => None,
                })
                .collect()
        } else {
            vec![CompressionAlgo::Zstd, CompressionAlgo::Brotli, CompressionAlgo::Gzip]
        };

        let min_body_size = cfg
            .get("min_body_size")
            .and_then(|v| v.as_u64())
            .map(|v| v as usize)
            .unwrap_or(DEFAULT_MIN_BODY_SIZE);

        let content_types = if let Some(arr) = cfg.get("content_types").and_then(|v| v.as_array())
        {
            arr.iter()
                .filter_map(|v| v.as_str())
                .map(String::from)
                .collect()
        } else {
            DEFAULT_CONTENT_TYPES.iter().map(|s| s.to_string()).collect()
        };

        let gzip_level = cfg
            .get("gzip_level")
            .and_then(|v| v.as_u64())
            .unwrap_or(6) as u32;

        let zstd_level = cfg
            .get("zstd_level")
            .and_then(|v| v.as_i64())
            .unwrap_or(3) as i32;

        let brotli_level = cfg
            .get("brotli_level")
            .and_then(|v| v.as_u64())
            .unwrap_or(4) as u32;

        Ok(Self {
            algorithms,
            min_body_size,
            content_types,
            gzip_level,
            zstd_level,
            brotli_level,
        })
    }

    fn is_compressible_type(&self, content_type: &str) -> bool {
        let ct = content_type
            .split(';')
            .next()
            .unwrap_or(content_type)
            .trim();
        self.content_types.iter().any(|pattern| {
            if pattern.ends_with("/*") || pattern.ends_with('/') {
                let prefix = pattern.trim_end_matches('*');
                ct.starts_with(prefix)
            } else {
                ct.eq_ignore_ascii_case(pattern)
            }
        })
    }

    fn negotiate(&self, accept_encoding: &str) -> Option<CompressionAlgo> {
        let mut candidates: Vec<(&str, f32)> = accept_encoding
            .split(',')
            .filter_map(|part| {
                let part = part.trim();
                let mut iter = part.splitn(2, ";q=");
                let algo = iter.next()?.trim();
                let q: f32 = iter
                    .next()
                    .and_then(|s| s.trim().parse().ok())
                    .unwrap_or(1.0);
                if q > 0.0 {
                    Some((algo, q))
                } else {
                    None
                }
            })
            .collect();

        candidates.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        for (name, _) in &candidates {
            for algo in &self.algorithms {
                match algo {
                    CompressionAlgo::Zstd if *name == "zstd" => return Some(CompressionAlgo::Zstd),
                    CompressionAlgo::Brotli if *name == "br" => return Some(CompressionAlgo::Brotli),
                    CompressionAlgo::Gzip if *name == "gzip" => return Some(CompressionAlgo::Gzip),
                    _ => {}
                }
            }
        }

        None
    }
}

#[async_trait]
impl Plugin for ResponseCompressionPlugin {
    fn name(&self) -> &str {
        "response-compression"
    }

    async fn on_request(&self, req: &mut Request<Body>) -> Result<(), PluginError> {
        // Negotiate encoding from Accept-Encoding and stash in extensions.
        // The proxy handler bridges request extensions onto the response so
        // on_response can read the negotiated algorithm.
        let encoding = req
            .headers()
            .get(ACCEPT_ENCODING)
            .and_then(|v| v.to_str().ok())
            .and_then(|ae| self.negotiate(ae));

        if let Some(algo) = encoding {
            req.extensions_mut().insert(NegotiatedEncoding(algo));
        }
        Ok(())
    }

    async fn on_response(&self, resp: &mut Response<BoxBody>) -> Result<(), PluginError> {
        let algo = match resp.extensions().get::<NegotiatedEncoding>() {
            Some(ne) => ne.0,
            None => return Ok(()),
        };

        if resp.headers().contains_key(CONTENT_ENCODING) {
            return Ok(());
        }

        let compressible = resp
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(|ct| self.is_compressible_type(ct))
            .unwrap_or(false);
        if !compressible {
            return Ok(());
        }

        let body = std::mem::replace(resp.body_mut(), Body::empty());
        let bytes = match body.collect().await {
            Ok(c) => c.to_bytes(),
            Err(_) => return Ok(()),
        };

        if bytes.len() < self.min_body_size {
            *resp.body_mut() = Body::from(bytes);
            return Ok(());
        }

        let (compressed, encoding_name) = match algo {
            CompressionAlgo::Gzip => {
                let mut enc = flate2::write::GzEncoder::new(
                    Vec::with_capacity(bytes.len() / 2),
                    flate2::Compression::new(self.gzip_level),
                );
                enc.write_all(&bytes)
                    .map_err(|e| PluginError::Internal(format!("gzip compress: {e}")))?;
                let out = enc
                    .finish()
                    .map_err(|e| PluginError::Internal(format!("gzip finish: {e}")))?;
                (out, "gzip")
            }
            CompressionAlgo::Brotli => {
                let mut out = Vec::with_capacity(bytes.len() / 2);
                {
                    let mut writer = brotli::CompressorWriter::new(
                        &mut out,
                        4096,
                        self.brotli_level,
                        22,
                    );
                    writer.write_all(&bytes)
                        .map_err(|e| PluginError::Internal(format!("brotli compress: {e}")))?;
                }
                (out, "br")
            }
            CompressionAlgo::Zstd => {
                let out = zstd::encode_all(bytes.as_ref(), self.zstd_level)
                    .map_err(|e| PluginError::Internal(format!("zstd compress: {e}")))?;
                (out, "zstd")
            }
        };

        if compressed.len() >= bytes.len() {
            *resp.body_mut() = Body::from(bytes);
            return Ok(());
        }

        resp.headers_mut()
            .insert(CONTENT_ENCODING, HeaderValue::from_static(encoding_name));
        resp.headers_mut().remove(CONTENT_LENGTH);
        resp.headers_mut()
            .append(VARY, HeaderValue::from_static("Accept-Encoding"));
        *resp.body_mut() = Body::from(compressed);
        Ok(())
    }
}
