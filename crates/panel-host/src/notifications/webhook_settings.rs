use super::{
    outbox,
    webhook::{self, Config, Preset},
};
use crate::{
    AppState,
    error::{ApiError, ApiResult},
    settings,
};
use axum::Json;
use serde::Deserialize;
use serde_json::{Value, json};

pub(super) fn view(config: Option<&Config>) -> Value {
    json!({
        "enabled": config.is_some_and(|value| value.enabled),
        "preset": config.map(|value| value.preset).unwrap_or_default(),
        "url_configured": config.is_some_and(|value| !value.url.is_empty()),
        "headers_configured": config.is_some_and(|value| !value.headers.is_empty()),
        "body_configured": config.is_some_and(|value| !value.body.is_empty()),
    })
}

pub async fn current(state: &AppState) -> ApiResult<Json<Value>> {
    Ok(Json(view(
        settings::read(&state.pool).await?.webhook.as_ref(),
    )))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Update {
    enabled: bool,
    preset: Preset,
    url: Option<String>,
    headers: Option<String>,
    body: Option<String>,
    #[serde(default)]
    clear_headers: bool,
}

pub async fn apply(state: &AppState, update: Update) -> ApiResult<Json<Value>> {
    let mut tx = state.pool.begin().await?;
    let mut settings = settings::lock(&mut tx).await?;
    // Changing provider never inherits credentials or templates from the previous provider.
    let previous = settings
        .webhook
        .as_ref()
        .filter(|value| value.preset == update.preset);
    let pick = |new: Option<String>, old: Option<&String>| {
        new.filter(|value| !value.trim().is_empty())
            .or_else(|| old.cloned())
            .unwrap_or_default()
    };
    let headers_supplied = update
        .headers
        .as_deref()
        .is_some_and(|value| !value.trim().is_empty());
    let config = Config {
        enabled: update.enabled,
        preset: update.preset,
        url: pick(
            update.url.map(|value| value.trim().to_owned()),
            previous.map(|value| &value.url),
        ),
        headers: if update.clear_headers {
            String::new()
        } else {
            pick(update.headers, previous.map(|value| &value.headers))
        },
        body: pick(update.body, previous.map(|value| &value.body)),
    };
    // Stored headers usually carry credentials; they never follow the URL to another host.
    if !headers_supplied
        && !update.clear_headers
        && previous
            .is_some_and(|value| !value.headers.is_empty() && !same_origin(&value.url, &config.url))
    {
        return Err(ApiError::BadRequest(
            "更换到其他主机时须重新填写或清除请求头，已保存的请求头不会发往新地址".into(),
        ));
    }
    webhook::validate(&config).map_err(|message| ApiError::BadRequest(message.into()))?;
    if settings.webhook.as_ref() != Some(&config) {
        sqlx::query("DELETE FROM notification_channel_tests WHERE channel='webhook'")
            .execute(&mut *tx)
            .await?;
    }
    let result = view(Some(&config));
    let old = settings.clone();
    settings.webhook = Some(config);
    outbox::settings_changed(&mut tx, &old, &settings).await?;
    settings::store(&mut tx, &settings).await?;
    tx.commit().await?;
    Ok(Json(result))
}

fn same_origin(left: &str, right: &str) -> bool {
    match (reqwest::Url::parse(left), reqwest::Url::parse(right)) {
        (Ok(left), Ok(right)) => left.origin() == right.origin(),
        _ => false,
    }
}

pub async fn clear(state: &AppState) -> ApiResult<Json<Value>> {
    let mut tx = state.pool.begin().await?;
    let mut settings = settings::lock(&mut tx).await?;
    let old = settings.clone();
    settings.webhook = None;
    settings::store(&mut tx, &settings).await?;
    outbox::settings_changed(&mut tx, &old, &settings).await?;
    sqlx::query("DELETE FROM notification_channel_tests WHERE channel='webhook'")
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Json(view(None)))
}
