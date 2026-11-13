//! The panel's own cookies (sessions and the bindings of sign-in ceremonies): reading one by
//! name and setting one for the whole panel, HTTP-only, same-site strict and, when the
//! panel is served over HTTPS, secure.

use crate::{
    AppState,
    error::{ApiError, ApiResult},
};
use axum::{
    http::{HeaderMap, HeaderValue, header},
    response::Response,
};

/// The value of cookie `name` across every `Cookie` header. None when it is missing, empty,
/// longer than 512 bytes or sent more than once, so a second copy cannot choose which
/// value counts.
pub fn read<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    let mut found = None;
    for header in headers.get_all(header::COOKIE) {
        for item in header.to_str().ok()?.split(';') {
            if let Some((key, value)) = item.trim().split_once('=')
                && key == name
            {
                if found.is_some() || value.is_empty() || value.len() > 512 {
                    return None;
                }
                found = Some(value);
            }
        }
    }
    found
}

/// Adds cookie `name` with `value` to `response`, valid for `lifetime` seconds; an empty
/// value with a lifetime of zero removes it.
pub fn set(
    state: &AppState,
    response: &mut Response,
    name: &str,
    value: &str,
    lifetime: i64,
) -> ApiResult<()> {
    let secure = if state.config.public_url.starts_with("https://") {
        "; Secure"
    } else {
        ""
    };
    response.headers_mut().append(
        header::SET_COOKIE,
        HeaderValue::from_str(&format!(
            "{name}={value}; Path=/; HttpOnly; SameSite=Strict; Max-Age={lifetime}{secure}"
        ))
        .map_err(|error| ApiError::Internal(error.into()))?,
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::read;
    use axum::http::{HeaderMap, HeaderValue, header};

    #[test]
    fn a_cookie_is_read_once_by_its_own_name() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            HeaderValue::from_static(
                "apsp_session=TEST_ONLY_ADMIN; apsp_binding=TEST_ONLY_BINDING",
            ),
        );
        assert_eq!(read(&headers, "apsp_binding"), Some("TEST_ONLY_BINDING"));
        assert_eq!(read(&headers, "apsp_session"), Some("TEST_ONLY_ADMIN"));
        assert_eq!(read(&headers, "missing"), None);
        // A second copy, in the same header or another one, refuses both.
        headers.append(
            header::COOKIE,
            HeaderValue::from_static("apsp_binding=TEST_ONLY_SECOND"),
        );
        assert!(read(&headers, "apsp_binding").is_none());
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            HeaderValue::from_static("apsp_session=a; apsp_session=b"),
        );
        assert!(read(&headers, "apsp_session").is_none());
        let mut headers = HeaderMap::new();
        headers.insert(header::COOKIE, HeaderValue::from_static("apsp_session="));
        assert!(read(&headers, "apsp_session").is_none());
        let long = format!("apsp_session={}", "a".repeat(513));
        headers.insert(header::COOKIE, HeaderValue::from_str(&long).unwrap());
        assert!(read(&headers, "apsp_session").is_none());
    }
}
