use axum::body::Body;
use axum::http::{header, Response, StatusCode};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use tokio::fs;

static HASHED_ASSET_RE: OnceLock<regex::Regex> = OnceLock::new();

fn hashed_re() -> &'static regex::Regex {
    HASHED_ASSET_RE.get_or_init(|| {
        regex::Regex::new(
            r"[.\-][0-9a-f]{8,}\.(js|css|woff2?|ttf|eot|svg|png|jpe?g|gif|webp|avif|ico)$",
        )
        .unwrap()
    })
}

fn mime_for_ext(ext: &str) -> &'static str {
    match ext {
        "html" | "htm" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" | "mjs" => "application/javascript; charset=utf-8",
        "json" => "application/json; charset=utf-8",
        "xml" => "application/xml; charset=utf-8",
        "txt" => "text/plain; charset=utf-8",
        "csv" => "text/csv; charset=utf-8",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        "ico" => "image/x-icon",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "eot" => "application/vnd.ms-fontobject",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "mp3" => "audio/mpeg",
        "ogg" => "audio/ogg",
        "pdf" => "application/pdf",
        "wasm" => "application/wasm",
        "map" => "application/json",
        "gz" => "application/gzip",
        "br" => "application/x-brotli",
        _ => "application/octet-stream",
    }
}

fn is_static_asset(ext: &str) -> bool {
    matches!(
        ext,
        "js" | "mjs"
            | "css"
            | "woff2"
            | "woff"
            | "ttf"
            | "otf"
            | "eot"
            | "svg"
            | "png"
            | "jpg"
            | "jpeg"
            | "gif"
            | "webp"
            | "avif"
            | "ico"
            | "mp4"
            | "webm"
            | "mp3"
            | "ogg"
            | "pdf"
            | "wasm"
    )
}

fn empty_response(status: StatusCode) -> Response<Body> {
    Response::builder()
        .status(status)
        .body(Body::empty())
        .unwrap()
}

fn file_response(
    bytes: bytes::Bytes,
    ext: &str,
    request_path: &str,
    content_encoding: Option<&str>,
) -> Response<Body> {
    let mime = mime_for_ext(ext);

    let cache_control = if hashed_re().is_match(request_path) {
        "public, max-age=31536000, immutable"
    } else if ext == "html" || ext == "htm" || request_path == "/" {
        "public, max-age=0, must-revalidate"
    } else if is_static_asset(ext) {
        "public, max-age=86400, stale-while-revalidate=604800"
    } else {
        "public, max-age=3600"
    };

    let mut builder = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, mime)
        .header(header::CONTENT_LENGTH, bytes.len())
        .header(header::CACHE_CONTROL, cache_control)
        .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff")
        .header("X-Frame-Options", "DENY")
        .header("Referrer-Policy", "strict-origin-when-cross-origin");

    if let Some(enc) = content_encoding {
        builder = builder
            .header(header::CONTENT_ENCODING, enc)
            .header(header::VARY, "Accept-Encoding");
    }

    builder.body(Body::from(bytes)).unwrap()
}

/// Resolve `request_path` to a file under `root`, blocking path traversal.
fn safe_resolve(root: &Path, request_path: &str) -> Option<PathBuf> {
    let clean = request_path.trim_start_matches('/');
    if clean.contains("..") {
        return None;
    }
    let candidate = root.join(clean);
    // Canonicalize only if it exists; we check existence separately.
    // This avoids a TOCTOU but keeps the traversal check.
    if candidate
        .components()
        .any(|c| c == std::path::Component::ParentDir)
    {
        return None;
    }
    Some(candidate)
}

pub async fn serve(
    document_root: &str,
    index_file: &str,
    spa_fallback: bool,
    request_path: &str,
    accept_encoding: &str,
) -> Response<Body> {
    let root = PathBuf::from(document_root);
    let encodings = parse_accept_encoding(accept_encoding);

    let candidate = match safe_resolve(&root, request_path) {
        Some(p) => p,
        None => return empty_response(StatusCode::FORBIDDEN),
    };

    if let Some(resp) = try_serve_file(&candidate, request_path, &encodings).await {
        return resp;
    }

    if candidate.extension().is_none() || request_path.ends_with('/') {
        let index = candidate.join(index_file);
        if let Some(resp) = try_serve_file(&index, request_path, &encodings).await {
            return resp;
        }
    }

    if request_path == "/favicon.ico" {
        return empty_response(StatusCode::NO_CONTENT);
    }

    if spa_fallback {
        let index = root.join(index_file);
        if let Some(resp) = try_serve_file(&index, "/", &encodings).await {
            return resp;
        }
    }

    empty_response(StatusCode::NOT_FOUND)
}

struct PrecompressedVariant {
    suffix: &'static str,
    encoding: &'static str,
}

const PRECOMPRESSED: &[PrecompressedVariant] = &[
    PrecompressedVariant { suffix: ".br",  encoding: "br" },
    PrecompressedVariant { suffix: ".zst", encoding: "zstd" },
    PrecompressedVariant { suffix: ".gz",  encoding: "gzip" },
];

fn parse_accept_encoding(header: &str) -> Vec<String> {
    header
        .split(',')
        .map(|s| s.split(';').next().unwrap_or("").trim().to_lowercase())
        .filter(|s| !s.is_empty())
        .collect()
}

async fn try_serve_file(
    path: &Path,
    request_path: &str,
    accept_encodings: &[String],
) -> Option<Response<Body>> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");

    for variant in PRECOMPRESSED {
        if accept_encodings.iter().any(|e| e == variant.encoding) {
            let compressed_path = path.with_extension(format!("{}{}", ext, variant.suffix));
            if let Ok(meta) = fs::metadata(&compressed_path).await {
                if meta.is_file() {
                    if let Ok(bytes) = fs::read(&compressed_path).await {
                        return Some(file_response(
                            bytes::Bytes::from(bytes),
                            ext,
                            request_path,
                            Some(variant.encoding),
                        ));
                    }
                }
            }
        }
    }

    let meta = fs::metadata(path).await.ok()?;
    if !meta.is_file() {
        return None;
    }
    let bytes = fs::read(path).await.ok()?;
    Some(file_response(bytes::Bytes::from(bytes), ext, request_path, None))
}
