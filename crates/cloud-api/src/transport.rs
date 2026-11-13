use super::Failure;
use apsp_panel_host::http::{self, BodyError, Outbound, Refusal, RetryAfter};
use reqwest::{Client, RequestBuilder, Response, StatusCode};
use serde_json::Value;
use std::{
    sync::OnceLock,
    time::{Duration, SystemTime},
};

/// How long a provider may make the panel wait after a 429: one to sixty minutes.
pub const RETRY_AFTER: RetryAfter = RetryAfter::new(60, 3600);
/// The wait after a 429 that names none, or none the panel can read.
const DEFAULT_RETRY_AFTER: i64 = 300;
/// The largest provider answer the panel reads, unless a client states its own limit.
pub const LIMIT: usize = 256 * 1024;

/// The client every cloud provider call shares. Background loops create their
/// provider handles every few seconds; one client keeps the TLS setup and
/// kept-alive connections across those ticks. Cloning is a reference count.
pub fn client() -> Result<Client, Failure> {
    static CLIENT: OnceLock<Client> = OnceLock::new();
    if let Some(client) = CLIENT.get() {
        return Ok(client.clone());
    }
    let client = Outbound {
        connect_timeout: Some(Duration::from_secs(3)),
        timeout: Some(Duration::from_secs(6)),
        ..Outbound::default()
    }
    .build()
    .map_err(|_| Failure::from("client_error"))?;
    Ok(CLIENT.get_or_init(|| client).clone())
}

pub async fn json(request: RequestBuilder) -> Result<Value, Failure> {
    json_inner(request, false).await
}
pub async fn power_json(request: RequestBuilder) -> Result<Value, Failure> {
    json_inner(request, true).await
}
pub fn power_rejection(value: &Value) -> Option<&'static str> {
    match value["Code"].as_str()? {
        "OperationDenied.NoStock"
        | "Invalid.PrivatePoolOptions.NoStock"
        | "LackResource"
        | "OperationDenied.SpotPriceLowerThanPublicPrice" => Some("capacity_unavailable"),
        "InsufficientBalance" | "InstanceExpired" | "DiskInArrears" => Some("insufficient_balance"),
        "InstanceLockedForSecurity" => Some("resource_locked"),
        "InvalidInstanceId.NotFound" => Some("resource_not_found"),
        "IncorrectInstanceStatus" | "InvalidParameter" | "Forbidden.RAM" => {
            Some("request_rejected")
        }
        _ => None,
    }
}
async fn json_inner(request: RequestBuilder, power: bool) -> Result<Value, Failure> {
    let response = request
        .send()
        .await
        .map_err(|_| Failure::from("network_error"))?;
    let status = response.status();
    // Power actions explain a refused request in the body, so their 4xx answers are read.
    let explained = power && status.is_client_error() && status != StatusCode::TOO_MANY_REQUESTS;
    if !explained && let Some(failure) = refusal(&response, "provider_rejected") {
        return Err(failure);
    }
    let value = bounded_json(response, LIMIT).await?;
    if power && let Some(code) = power_rejection(&value) {
        return Err(code.into());
    }
    if !status.is_success() {
        return Err("provider_rejected".into());
    }
    Ok(value)
}

/// The failure a provider's answer stands for by its status, or `None` when it succeeded.
/// `other` names a status outside the shared categories. A 429 waits as long as the
/// provider asks (one to sixty minutes).
pub fn refusal(response: &Response, other: &'static str) -> Option<Failure> {
    let category = Refusal::of(response.status())?;
    let mut failure = refused(category, other);
    if category == Refusal::RateLimited
        && let Some(wait) = RETRY_AFTER.header(response.headers(), SystemTime::now())
    {
        failure.retry_after = wait;
    }
    Some(failure)
}

/// The failure a category of refused status stands for, when only the status is known: a
/// 429 waits the default time. `other` names a status outside the shared categories.
pub fn refused(category: Refusal, other: &'static str) -> Failure {
    match category {
        Refusal::RateLimited => Failure {
            code: "rate_limited",
            retry_after: DEFAULT_RETRY_AFTER,
        },
        Refusal::Credentials => "authentication_failed".into(),
        Refusal::Missing => "resource_missing".into(),
        Refusal::Rejected => "provider_rejected".into(),
        Refusal::Redirect => "redirect_refused".into(),
        Refusal::Unavailable => "provider_unavailable".into(),
        Refusal::Other => other.into(),
    }
}

/// Reads a JSON body of at most `limit` bytes, whatever the declared length says.
pub async fn bounded_json(response: Response, limit: usize) -> Result<Value, Failure> {
    http::json(response, limit).await.map_err(|error| {
        Failure::from(match error {
            BodyError::TooLarge => "response_too_large",
            BodyError::Read(_) => "network_error",
            BodyError::Invalid => "invalid_response",
        })
    })
}
