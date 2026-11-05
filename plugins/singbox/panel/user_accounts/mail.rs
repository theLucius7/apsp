use super::*;
use axum::{Json, extract::State};
use lettre::{
    AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor,
    transport::smtp::authentication::Credentials,
};

#[derive(Clone, Deserialize, Serialize, sqlx::FromRow)]
#[serde(deny_unknown_fields)]
pub(super) struct Configuration {
    pub enabled: bool,
    pub reminders_enabled: bool,
    pub host: String,
    pub port: i32,
    pub security: String,
    pub username: String,
    pub from_address: String,
    pub credential_id: Option<Uuid>,
    pub blocked_domains: Vec<String>,
    pub version: i64,
}
pub(super) async fn config(state: &AppState) -> ApiResult<Configuration> {
    Ok(sqlx::query_as("SELECT enabled,reminders_enabled,host,port,security,username,from_address,credential_id,blocked_domains,version FROM singbox_mail_settings WHERE singleton").fetch_one(&state.pool).await?)
}
pub(super) async fn settings(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> ApiResult<Json<Configuration>> {
    administrator(&state, &headers, false).await?;
    Ok(Json(config(&state).await?))
}
pub(super) async fn save(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<Configuration>,
) -> ApiResult<Json<Value>> {
    let actor = administrator(&state, &headers, true).await?;
    if !["tls", "starttls"].contains(&input.security.as_str())
        || !(1..=65535).contains(&input.port)
        || input.blocked_domains.len() > 200
    {
        return Err(bad("邮件端口、加密方式或限制域名无效"));
    }
    if input.enabled
        && (input.host.is_empty()
            || input.host.len() > 253
            || input
                .host
                .chars()
                .any(|c| !c.is_ascii_alphanumeric() && c != '-' && c != '.')
            || email(&input.from_address).is_err())
    {
        return Err(bad("启用邮件需要有效的 SMTP 主机与发件地址"));
    }
    text(&input.username, 254, false)?;
    for domain in &input.blocked_domains {
        if domain.len() > 253
            || domain.is_empty()
            || domain != &domain.to_lowercase()
            || !domain
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'.')
        {
            return Err(bad("限制域名格式无效"));
        }
    }
    let stored = config(&state).await?;
    // The credential is sent as a password to the configured host, so choosing it or
    // redirecting it to another server is reserved for the owner.
    let redirected = input.credential_id.is_some()
        && (input.credential_id != stored.credential_id
            || input.host != stored.host
            || input.port != stored.port
            || input.security != stored.security);
    crate::control_center::require_credential_choice(&state, &headers, redirected).await?;
    if input.enabled && !input.username.is_empty() {
        let id = input
            .credential_id
            .ok_or_else(|| bad("SMTP 认证需要凭据引用"))?;
        secret(
            &crate::control_center::credentials::resolve_reference(
                &state,
                id,
                "external-api",
                "member-mail-settings",
            )
            .await?,
        )?;
    }
    let mut tx = state.pool.begin().await?;
    let changed=sqlx::query("UPDATE singbox_mail_settings SET enabled=$1,reminders_enabled=$2,host=$3,port=$4,security=$5,username=$6,from_address=$7,credential_id=$8,blocked_domains=$9,version=version+1 WHERE singleton AND version=$10").bind(input.enabled).bind(input.reminders_enabled).bind(&input.host).bind(input.port).bind(&input.security).bind(&input.username).bind(&input.from_address).bind(input.credential_id).bind(&input.blocked_domains).bind(input.version).execute(&mut *tx).await?.rows_affected();
    if changed != 1 {
        return Err(conflict("邮件配置已更新，请刷新"));
    }
    if !input.enabled || !input.reminders_enabled {
        sqlx::query("UPDATE singbox_member_messages SET mail_state='cancelled' WHERE mail_state IN('pending','failed')").execute(&mut *tx).await?;
    }
    event(
        &mut tx,
        None,
        &actor,
        "mail_settings_changed",
        "mail",
        json!({"enabled":input.enabled,"reminders_enabled":input.reminders_enabled}),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(json!({"ok":true})))
}
fn secret(value: &Value) -> ApiResult<String> {
    value["key"]
        .as_str()
        .filter(|s| !s.is_empty() && s.len() <= 4096)
        .map(str::to_owned)
        .ok_or_else(|| bad("SMTP 凭据须包含 key 字段"))
}
pub(super) fn allowed(config: &Configuration, address: &str) -> bool {
    let domain = address.rsplit('@').next().unwrap_or_default();
    config.enabled
        && !config
            .blocked_domains
            .iter()
            .any(|d| domain == d || domain.ends_with(&format!(".{d}")))
}
pub(super) async fn send(
    state: &AppState,
    config: &Configuration,
    address: &str,
    subject: &str,
    body: &str,
) -> ApiResult<()> {
    if !allowed(config, address) {
        return Err(conflict("当前邮件设置无法投递此地址"));
    }
    let builder = if config.security == "tls" {
        AsyncSmtpTransport::<Tokio1Executor>::relay(&config.host)
    } else {
        AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&config.host)
    }
    .map_err(|_| bad("SMTP TLS 配置无效"))?;
    let mut builder = builder
        .port(config.port as u16)
        .timeout(Some(std::time::Duration::from_secs(20)));
    if !config.username.is_empty() {
        let id = config
            .credential_id
            .ok_or_else(|| bad("SMTP 认证凭据缺失"))?;
        let value = crate::control_center::credentials::resolve_reference(
            state,
            id,
            "external-api",
            "member-mail-delivery",
        )
        .await?;
        builder = builder.credentials(Credentials::new(config.username.clone(), secret(&value)?));
    }
    let message = Message::builder()
        .from(
            config
                .from_address
                .parse()
                .map_err(|_| bad("发件地址无效"))?,
        )
        .to(address.parse().map_err(|_| bad("收件地址无效"))?)
        .subject(subject)
        .body(body.to_owned())
        .map_err(|_| bad("邮件内容无效"))?;
    tokio::time::timeout(
        std::time::Duration::from_secs(30),
        builder.build().send(message),
    )
    .await
    .map_err(|_| conflict("邮件投递超时，请稍后重试"))?
    .map_err(|_| conflict("邮件投递未完成，请核对 SMTP 配置或稍后重试"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn domain_policy_matches_subdomains_only() {
        let config = Configuration {
            enabled: true,
            reminders_enabled: false,
            host: String::new(),
            port: 465,
            security: "tls".into(),
            username: String::new(),
            from_address: String::new(),
            credential_id: None,
            blocked_domains: vec!["blocked.invalid".into()],
            version: 1,
        };
        assert!(!allowed(&config, "member@sub.blocked.invalid"));
        assert!(allowed(&config, "member@notblocked.invalid"));
    }
}
