use std::{borrow::Cow, fmt::Write as _};

use axum::{
    body::{Body, Bytes},
    http::{HeaderMap, HeaderValue, Method, StatusCode, Uri, header},
    response::{IntoResponse, Response},
};
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "../../web/dist/"]
struct Assets;

pub async fn serve(method: Method, uri: Uri, headers: HeaderMap) -> Response {
    let path = uri.path().strip_prefix('/').unwrap_or(uri.path());
    let path = if path.is_empty() { "index.html" } else { path };
    // The UI uses hash routes. Unknown endpoints and missing assets must stay errors.
    if path == "api" || path.starts_with("api/") {
        return not_found();
    }
    let Some(asset) = Assets::get(path) else {
        return not_found();
    };
    if method != Method::GET && method != Method::HEAD {
        return (
            StatusCode::METHOD_NOT_ALLOWED,
            [(header::ALLOW, "GET, HEAD")],
            "不支持的请求方法",
        )
            .into_response();
    }
    let (cache, revalidated) = if path == "index.html" {
        ("no-store", false)
    } else if is_hashed_asset(path) {
        ("public, max-age=31536000, immutable", false)
    } else {
        ("no-cache", true)
    };
    // Unhashed files must be revalidated; a validator turns that into a 304.
    let etag = revalidated.then(|| etag(&asset.metadata.sha256_hash()));
    if let Some(etag) = &etag
        && if_none_match(&headers, etag)
    {
        return (
            StatusCode::NOT_MODIFIED,
            [
                (header::ETAG, etag.clone()),
                (header::CACHE_CONTROL, HeaderValue::from_static(cache)),
            ],
        )
            .into_response();
    }
    let length = HeaderValue::from(asset.data.len());
    let body = if method == Method::HEAD {
        Body::empty()
    } else {
        // Embedded files are static: serve them without copying.
        match asset.data {
            Cow::Borrowed(data) => Body::from(Bytes::from_static(data)),
            Cow::Owned(data) => Body::from(data),
        }
    };
    let mut response = (
        [
            (
                header::CONTENT_TYPE,
                HeaderValue::from_static(content_type(path)),
            ),
            (header::CACHE_CONTROL, HeaderValue::from_static(cache)),
            (header::CONTENT_LENGTH, length),
            (
                header::X_CONTENT_TYPE_OPTIONS,
                HeaderValue::from_static("nosniff"),
            ),
        ],
        body,
    )
        .into_response();
    if let Some(etag) = etag {
        response.headers_mut().insert(header::ETAG, etag);
    }
    response
}

fn etag(digest: &[u8]) -> HeaderValue {
    let mut value = String::with_capacity(digest.len() * 2 + 2);
    value.push('"');
    for byte in digest {
        let _ = write!(value, "{byte:02x}");
    }
    value.push('"');
    HeaderValue::from_str(&value).expect("a quoted hex digest is a valid header value")
}

fn if_none_match(headers: &HeaderMap, etag: &HeaderValue) -> bool {
    let Some(etag) = etag.to_str().ok() else {
        return false;
    };
    headers
        .get_all(header::IF_NONE_MATCH)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .map(str::trim)
        .any(|candidate| candidate == "*" || candidate.trim_start_matches("W/") == etag)
}

fn not_found() -> Response {
    (
        StatusCode::NOT_FOUND,
        [
            (header::CACHE_CONTROL, "no-store"),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        ],
        "资源不存在",
    )
        .into_response()
}

fn is_hashed_asset(path: &str) -> bool {
    let Some(stem) = path
        .strip_prefix("assets/")
        .and_then(|path| path.rsplit_once('.').map(|(stem, _)| stem))
    else {
        return false;
    };
    // Vite's default content hash is eight URL-safe characters, including '_' and '-'.
    let bytes = stem.as_bytes();
    bytes.len() > 9
        && bytes[bytes.len() - 9] == b'-'
        && bytes[bytes.len() - 8..]
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

fn content_type(path: &str) -> &'static str {
    match path.rsplit('.').next().unwrap_or_default() {
        "html" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" | "map" => "application/json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "wasm" => "application/wasm",
        "txt" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}
