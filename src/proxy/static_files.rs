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

/// Default patterns for files that should be hidden from static serving.
const DEFAULT_HIDDEN: &[&str] = &[
    ".git",
    ".env",
    ".DS_Store",
    ".htaccess",
    ".htpasswd",
    "Thumbs.db",
    "desktop.ini",
    ".svn",
    ".hg",
];

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

/// Generate a strong ETag from file size and modification time.
fn generate_etag(size: u64, modified: std::time::SystemTime) -> String {
    let epoch_secs = modified
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    format!("\"{:x}-{:x}\"", size, epoch_secs)
}

/// Format a SystemTime as an HTTP-date (RFC 7231).
fn format_http_date(t: std::time::SystemTime) -> String {
    let d = t
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = d.as_secs();
    // We do simple HTTP-date formatting without pulling in httpdate crate.
    // Format: "Mon, 01 Jan 2024 00:00:00 GMT"
    let days = secs / 86400;
    let time_of_day = secs % 86400;
    let hours = time_of_day / 3600;
    let minutes = (time_of_day % 3600) / 60;
    let seconds = time_of_day % 60;

    // Days since epoch (1970-01-01 Thursday)
    let wday = ((days + 4) % 7) as usize; // 0=Sunday
    let wday_names = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];

    // Compute year/month/day from days since epoch
    let (year, month, day) = days_to_ymd(days as i64);
    let month_names = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];

    format!(
        "{}, {:02} {} {} {:02}:{:02}:{:02} GMT",
        wday_names[wday],
        day,
        month_names[(month - 1) as usize],
        year,
        hours,
        minutes,
        seconds
    )
}

fn days_to_ymd(days: i64) -> (i64, i64, i64) {
    // Civil days to year/month/day (Euclidean affine algorithm).
    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

/// Check If-None-Match / If-Modified-Since for 304 eligibility.
fn check_conditional(
    if_none_match: Option<&str>,
    if_modified_since: Option<&str>,
    etag: &str,
    _modified: std::time::SystemTime,
) -> bool {
    // ETag match takes priority.
    if let Some(inm) = if_none_match {
        return inm
            .split(',')
            .any(|tag| tag.trim().trim_matches('"') == etag.trim_matches('"'));
    }
    // Fallback to If-Modified-Since.
    if let Some(_ims) = if_modified_since {
        // Simplified: if the ETag matches, return 304. For If-Modified-Since
        // we'd need HTTP-date parsing; skipping for now since ETag is the
        // primary mechanism and all our responses include ETag.
        return false;
    }
    false
}

/// Parse Range header, returning (start, end) byte offsets.
/// Supports single byte ranges: "bytes=0-499", "bytes=500-", "bytes=-500".
fn parse_range(range_header: &str, file_size: u64) -> Option<(u64, u64)> {
    let range = range_header.strip_prefix("bytes=")?;
    let (start_str, end_str) = range.split_once('-')?;

    if start_str.is_empty() {
        // Suffix range: bytes=-500
        let suffix: u64 = end_str.parse().ok()?;
        if suffix > file_size {
            return None;
        }
        Some((file_size - suffix, file_size - 1))
    } else if end_str.is_empty() {
        // Open-ended: bytes=500-
        let start: u64 = start_str.parse().ok()?;
        if start >= file_size {
            return None;
        }
        Some((start, file_size - 1))
    } else {
        let start: u64 = start_str.parse().ok()?;
        let end: u64 = end_str.parse().ok()?;
        if start > end || start >= file_size {
            return None;
        }
        Some((start, end.min(file_size - 1)))
    }
}

fn file_response(
    bytes: bytes::Bytes,
    ext: &str,
    request_path: &str,
    content_encoding: Option<&str>,
    etag: Option<&str>,
    last_modified: Option<std::time::SystemTime>,
    cache_control_override: Option<&str>,
) -> Response<Body> {
    let mime = mime_for_ext(ext);

    let cache_control = cache_control_override.unwrap_or_else(|| {
        if hashed_re().is_match(request_path) {
            "public, max-age=31536000, immutable"
        } else if ext == "html" || ext == "htm" || request_path == "/" {
            "public, max-age=0, must-revalidate"
        } else if is_static_asset(ext) {
            "public, max-age=86400, stale-while-revalidate=604800"
        } else {
            "public, max-age=3600"
        }
    });

    let mut builder = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, mime)
        .header(header::CONTENT_LENGTH, bytes.len())
        .header(header::CACHE_CONTROL, cache_control)
        .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff")
        .header("X-Frame-Options", "DENY")
        .header("Referrer-Policy", "strict-origin-when-cross-origin")
        .header(header::ACCEPT_RANGES, "bytes");

    if let Some(tag) = etag {
        builder = builder.header(header::ETAG, tag);
    }
    if let Some(modified) = last_modified {
        builder = builder.header(header::LAST_MODIFIED, format_http_date(modified));
    }
    if let Some(enc) = content_encoding {
        builder = builder
            .header(header::CONTENT_ENCODING, enc)
            .header(header::VARY, "Accept-Encoding");
    }

    builder.body(Body::from(bytes)).unwrap()
}

fn range_response(
    bytes: bytes::Bytes,
    ext: &str,
    start: u64,
    end: u64,
    total: u64,
    etag: Option<&str>,
    content_encoding: Option<&str>,
) -> Response<Body> {
    let mime = mime_for_ext(ext);

    let mut builder = Response::builder()
        .status(StatusCode::PARTIAL_CONTENT)
        .header(header::CONTENT_TYPE, mime)
        .header(header::CONTENT_LENGTH, bytes.len())
        .header(
            header::CONTENT_RANGE,
            format!("bytes {start}-{end}/{total}"),
        )
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff");

    if let Some(tag) = etag {
        builder = builder.header(header::ETAG, tag);
    }
    if let Some(enc) = content_encoding {
        builder = builder.header(header::CONTENT_ENCODING, enc);
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
    if candidate
        .components()
        .any(|c| c == std::path::Component::ParentDir)
    {
        return None;
    }
    Some(candidate)
}

/// Check if a path component is hidden (dotfiles, .env, .git, etc.).
fn is_hidden_path(request_path: &str, extra_hide: &[String]) -> bool {
    for segment in request_path.split('/') {
        if segment.is_empty() {
            continue;
        }
        // Check default hidden patterns.
        for pattern in DEFAULT_HIDDEN {
            if segment == *pattern || segment.starts_with(&format!("{pattern}/")) {
                return true;
            }
        }
        // Check user-configured patterns.
        for pattern in extra_hide {
            if segment == pattern.as_str() {
                return true;
            }
        }
    }
    false
}

/// Static file serving options, passed from the service config.
#[derive(Default)]
pub struct ServeOptions<'a> {
    /// Extra file/directory names to hide (beyond defaults).
    pub hide_patterns: &'a [String],
    /// Override cache-control header value.
    pub cache_control: Option<&'a str>,
    /// Request headers for conditional/range handling.
    pub if_none_match: Option<&'a str>,
    pub if_modified_since: Option<&'a str>,
    pub range: Option<&'a str>,
}

pub async fn serve(
    document_root: &str,
    index_file: &str,
    spa_fallback: bool,
    request_path: &str,
    accept_encoding: &str,
) -> Response<Body> {
    serve_with_options(
        document_root,
        index_file,
        spa_fallback,
        request_path,
        accept_encoding,
        &ServeOptions::default(),
    )
    .await
}

pub async fn serve_with_options(
    document_root: &str,
    index_file: &str,
    spa_fallback: bool,
    request_path: &str,
    accept_encoding: &str,
    opts: &ServeOptions<'_>,
) -> Response<Body> {
    let root = PathBuf::from(document_root);
    let encodings = parse_accept_encoding(accept_encoding);

    // File hiding: reject hidden paths.
    if is_hidden_path(request_path, opts.hide_patterns) {
        return empty_response(StatusCode::NOT_FOUND);
    }

    let candidate = match safe_resolve(&root, request_path) {
        Some(p) => p,
        None => return empty_response(StatusCode::FORBIDDEN),
    };

    // Canonical path redirect: /dir → /dir/
    if !request_path.ends_with('/') && request_path != "/" {
        if let Ok(meta) = fs::metadata(&candidate).await {
            if meta.is_dir() {
                let redirect_to = format!("{request_path}/");
                return Response::builder()
                    .status(StatusCode::MOVED_PERMANENTLY)
                    .header(header::LOCATION, redirect_to)
                    .body(Body::empty())
                    .unwrap();
            }
        }
    }

    if let Some(resp) =
        try_serve_file(&candidate, request_path, &encodings, opts).await
    {
        return resp;
    }

    if candidate.extension().is_none() || request_path.ends_with('/') {
        let index = candidate.join(index_file);
        if let Some(resp) =
            try_serve_file(&index, request_path, &encodings, opts).await
        {
            return resp;
        }
    }

    if request_path == "/favicon.ico" {
        return empty_response(StatusCode::NO_CONTENT);
    }

    if spa_fallback {
        let index = root.join(index_file);
        if let Some(resp) =
            try_serve_file(&index, "/", &encodings, opts).await
        {
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
    PrecompressedVariant {
        suffix: ".br",
        encoding: "br",
    },
    PrecompressedVariant {
        suffix: ".zst",
        encoding: "zstd",
    },
    PrecompressedVariant {
        suffix: ".gz",
        encoding: "gzip",
    },
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
    opts: &ServeOptions<'_>,
) -> Option<Response<Body>> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");

    for variant in PRECOMPRESSED {
        if accept_encodings.iter().any(|e| e == variant.encoding) {
            let compressed_path = path.with_extension(format!("{}{}", ext, variant.suffix));
            if let Ok(meta) = fs::metadata(&compressed_path).await {
                if meta.is_file() {
                    if let Ok(bytes) = fs::read(&compressed_path).await {
                        let etag = generate_etag(meta.len(), meta.modified().unwrap_or(std::time::UNIX_EPOCH));
                        let modified = meta.modified().ok();

                        // Check conditional request.
                        if check_conditional(opts.if_none_match, opts.if_modified_since, &etag, modified.unwrap_or(std::time::UNIX_EPOCH)) {
                            return Some(
                                Response::builder()
                                    .status(StatusCode::NOT_MODIFIED)
                                    .header(header::ETAG, &etag)
                                    .body(Body::empty())
                                    .unwrap(),
                            );
                        }

                        return Some(file_response(
                            bytes::Bytes::from(bytes),
                            ext,
                            request_path,
                            Some(variant.encoding),
                            Some(&etag),
                            modified,
                            opts.cache_control,
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

    let file_size = meta.len();
    let modified = meta.modified().ok();
    let etag = generate_etag(file_size, modified.unwrap_or(std::time::UNIX_EPOCH));

    // Check conditional request (ETag / If-None-Match).
    if check_conditional(opts.if_none_match, opts.if_modified_since, &etag, modified.unwrap_or(std::time::UNIX_EPOCH)) {
        return Some(
            Response::builder()
                .status(StatusCode::NOT_MODIFIED)
                .header(header::ETAG, &etag)
                .body(Body::empty())
                .unwrap(),
        );
    }

    // Range request support.
    if let Some(range_header) = opts.range {
        if let Some((start, end)) = parse_range(range_header, file_size) {
            let bytes = fs::read(path).await.ok()?;
            let slice = bytes::Bytes::from(bytes[(start as usize)..=(end as usize)].to_vec());
            return Some(range_response(slice, ext, start, end, file_size, Some(&etag), None));
        } else {
            // Invalid range → 416 Range Not Satisfiable.
            return Some(
                Response::builder()
                    .status(StatusCode::RANGE_NOT_SATISFIABLE)
                    .header(header::CONTENT_RANGE, format!("bytes */{file_size}"))
                    .body(Body::empty())
                    .unwrap(),
            );
        }
    }

    let bytes = fs::read(path).await.ok()?;
    Some(file_response(
        bytes::Bytes::from(bytes),
        ext,
        request_path,
        None,
        Some(&etag),
        modified,
        opts.cache_control,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn etag_deterministic() {
        let t = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1700000000);
        let tag1 = generate_etag(1024, t);
        let tag2 = generate_etag(1024, t);
        assert_eq!(tag1, tag2);
        assert!(tag1.starts_with('"') && tag1.ends_with('"'));
    }

    #[test]
    fn etag_conditional_match() {
        let t = std::time::UNIX_EPOCH;
        let etag = generate_etag(100, t);
        assert!(check_conditional(Some(&etag), None, &etag, t));
        assert!(!check_conditional(Some("\"other\""), None, &etag, t));
        assert!(!check_conditional(None, None, &etag, t));
    }

    #[test]
    fn parse_range_cases() {
        assert_eq!(parse_range("bytes=0-499", 1000), Some((0, 499)));
        assert_eq!(parse_range("bytes=500-", 1000), Some((500, 999)));
        assert_eq!(parse_range("bytes=-200", 1000), Some((800, 999)));
        assert_eq!(parse_range("bytes=0-0", 1000), Some((0, 0)));
        assert_eq!(parse_range("bytes=999-", 1000), Some((999, 999)));
        assert_eq!(parse_range("bytes=1000-", 1000), None);
    }

    #[test]
    fn hidden_paths() {
        assert!(is_hidden_path("/.git/config", &[]));
        assert!(is_hidden_path("/.env", &[]));
        assert!(is_hidden_path("/.DS_Store", &[]));
        assert!(!is_hidden_path("/index.html", &[]));
        assert!(!is_hidden_path("/assets/app.js", &[]));
        assert!(is_hidden_path("/secret", &["secret".to_string()]));
    }

    #[test]
    fn canonical_redirect_candidate() {
        // Just test safe_resolve still works.
        let root = Path::new("/tmp");
        assert!(safe_resolve(root, "/foo").is_some());
        assert!(safe_resolve(root, "/../etc/passwd").is_none());
    }
}
