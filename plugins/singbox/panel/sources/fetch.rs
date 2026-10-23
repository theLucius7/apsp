//! Numbered-source adapter over the shared subscription fetcher.
//!
//! Keeps this stack's call shape, user agent and stored error codes; the
//! network policy lives in `crate::subscription_fetch`.

use super::parse::{ImportError, MAX_BODY};
use crate::subscription_fetch::{self as shared, Failure, FailureKind, parse_traffic};
use reqwest::Url;
use std::collections::BTreeMap;

const _: () = assert!(MAX_BODY == shared::MAX_BODY_BYTES);

pub(super) struct FetchResult {
    pub body: Option<Vec<u8>>,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub traffic: Option<serde_json::Value>,
}

pub(super) const DEFAULT_USER_AGENT: &str = "Apsp-subscription-import/1";

fn code(failure: Failure) -> ImportError {
    ImportError(match failure.kind {
        FailureKind::Url => "invalid_source_url",
        FailureKind::PrivateAddress | FailureKind::DnsEmpty | FailureKind::DnsLimit => {
            "non_public_source_address"
        }
        FailureKind::AuthHeaders => "invalid_source_authorization",
        FailureKind::UserAgent => "invalid_source_user_agent",
        FailureKind::Cache => "invalid_source_request",
        FailureKind::UnexpectedNotModified => "unexpected_not_modified",
        FailureKind::Dns => "source_dns_failed",
        FailureKind::Timeout => "download_timeout",
        FailureKind::Tls
        | FailureKind::Connection
        | FailureKind::Read
        | FailureKind::InvalidLength => "download_failed",
        FailureKind::Client => "download_client_failed",
        FailureKind::RedirectLimit => "redirect_limit",
        FailureKind::Redirect => "invalid_redirect",
        FailureKind::RedirectOrigin => "cross_origin_redirect",
        FailureKind::Http => "source_http_failed",
        FailureKind::Html => "html_response",
        FailureKind::BodyLimit => "body_limit",
        FailureKind::DecompressedLimit => "decompressed_body_limit",
        FailureKind::UnsupportedEncoding | FailureKind::InvalidEncodingHeader => {
            "unsupported_content_encoding"
        }
        FailureKind::CorruptBody => "invalid_compressed_body",
    })
}

pub(super) fn validate_user_agent(value: &str) -> Result<(), ImportError> {
    shared::validate_user_agent(value).map_err(code)
}

pub(super) fn validate_url(value: &str) -> Result<Url, ImportError> {
    shared::validate_url(value).map_err(code)
}

fn auth_headers(authorization: Option<&str>) -> BTreeMap<String, String> {
    authorization
        .map(|value| BTreeMap::from([("authorization".to_owned(), value.to_owned())]))
        .unwrap_or_default()
}

pub(super) fn validate_authorization(value: &str) -> Result<(), ImportError> {
    shared::validate_auth_headers(&auth_headers(Some(value)))
        .map(drop)
        .map_err(code)
}

pub(super) async fn download(
    value: &str,
    authorization: Option<&str>,
    etag: Option<&str>,
    last_modified: Option<&str>,
    user_agent: &str,
) -> Result<FetchResult, ImportError> {
    let auth_headers = auth_headers(authorization);
    // Validators stored before the shared limits applied are dropped: the
    // source then downloads in full instead of failing on its own cache.
    let request = shared::Request {
        url: value,
        auth_headers: &auth_headers,
        etag: etag.filter(|value| shared::valid_cache_value(value)),
        last_modified: last_modified.filter(|value| shared::valid_cache_value(value)),
        user_agent: Some(user_agent),
    };
    Ok(match shared::fetch(&request).await.map_err(code)? {
        shared::Outcome::Modified {
            body,
            etag,
            last_modified,
            traffic,
        } => FetchResult {
            body: Some(body),
            etag,
            last_modified,
            traffic: traffic.as_deref().and_then(parse_traffic),
        },
        shared::Outcome::NotModified {
            etag,
            last_modified,
            traffic,
        } => FetchResult {
            body: None,
            etag,
            last_modified,
            traffic: traffic.as_deref().and_then(parse_traffic),
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stored_error_codes_stay_in_this_stack_vocabulary() {
        for (kind, expected) in [
            (FailureKind::Url, "invalid_source_url"),
            (FailureKind::PrivateAddress, "non_public_source_address"),
            (FailureKind::DnsEmpty, "non_public_source_address"),
            (FailureKind::DnsLimit, "non_public_source_address"),
            (FailureKind::AuthHeaders, "invalid_source_authorization"),
            (FailureKind::UserAgent, "invalid_source_user_agent"),
            (FailureKind::Cache, "invalid_source_request"),
            (
                FailureKind::UnexpectedNotModified,
                "unexpected_not_modified",
            ),
            (FailureKind::Dns, "source_dns_failed"),
            (FailureKind::Timeout, "download_timeout"),
            (FailureKind::Tls, "download_failed"),
            (FailureKind::Connection, "download_failed"),
            (FailureKind::Read, "download_failed"),
            (FailureKind::InvalidLength, "download_failed"),
            (FailureKind::Client, "download_client_failed"),
            (FailureKind::RedirectLimit, "redirect_limit"),
            (FailureKind::Redirect, "invalid_redirect"),
            (FailureKind::RedirectOrigin, "cross_origin_redirect"),
            (FailureKind::Http, "source_http_failed"),
            (FailureKind::Html, "html_response"),
            (FailureKind::BodyLimit, "body_limit"),
            (FailureKind::DecompressedLimit, "decompressed_body_limit"),
            (
                FailureKind::UnsupportedEncoding,
                "unsupported_content_encoding",
            ),
            (
                FailureKind::InvalidEncodingHeader,
                "unsupported_content_encoding",
            ),
            (FailureKind::CorruptBody, "invalid_compressed_body"),
        ] {
            let failure = Failure {
                kind,
                message: "固定失败说明",
                http_status: None,
            };
            assert_eq!(code(failure).0, expected);
        }
    }

    #[test]
    fn validation_maps_to_the_existing_codes() {
        for (url, expected) in [
            ("https://user:secret@example.com", "invalid_source_url"),
            ("http://example.com", "invalid_source_url"),
            ("https://localhost", "non_public_source_address"),
            ("https://127.1", "non_public_source_address"),
            ("https://metadata.internal", "non_public_source_address"),
        ] {
            assert_eq!(validate_url(url).unwrap_err().0, expected, "{url}");
        }
        assert!(validate_url("https://[2600::1]/subscription").is_ok());
        assert!(validate_authorization("Bearer fixture-secret").is_ok());
        let oversized = "x".repeat(8192);
        for invalid in ["", " ", "Bearer a\r\nHost: b", oversized.as_str()] {
            assert_eq!(
                validate_authorization(invalid).unwrap_err().0,
                "invalid_source_authorization"
            );
        }
        assert!(validate_user_agent(DEFAULT_USER_AGENT).is_ok());
        assert_eq!(
            validate_user_agent("agent\r\nAuthorization: secret")
                .unwrap_err()
                .0,
            "invalid_source_user_agent"
        );
    }
}
