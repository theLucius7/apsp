use crate::{
    AppState, business,
    error::{ApiError, ApiResult},
};
use apsp_compiler::server_profile::ServerProfile;
use axum::{
    Json,
    extract::{Path, State},
    http::HeaderMap,
};
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::{Postgres, Transaction};

const MASK: &str = "__APSP_REDACTED__";
/// Credential fields of server configuration objects. The administration guard's audit
/// redaction covers every name listed here.
pub(crate) fn secret(key: &str) -> bool {
    let normalized = key.to_ascii_lowercase();
    let key = normalized.as_str();
    [
        "token",
        "password",
        "uuid",
        "private_key",
        "key",
        "api_token",
        "psk",
        "certificate",
        "auth_key",
        "client_key",
        "cookie",
        "authorization",
        "secret",
        "static_key",
        "pre_shared_key",
    ]
    .contains(&key)
        || key.ends_with("_password")
        || key.ends_with("_token")
        || key.ends_with("_secret")
        || key.ends_with("_key")
}
fn redact(value: &mut Value) {
    match value {
        Value::Object(map) => {
            for (key, value) in map {
                if secret(key) && !value.is_null() {
                    *value = json!(MASK);
                } else {
                    redact(value);
                }
            }
        }
        Value::Array(items) => items.iter_mut().for_each(redact),
        _ => {}
    }
}
fn restore(value: &mut Value, previous: &Value) -> ApiResult<()> {
    match value {
        Value::Object(map) => {
            for (key, item) in map {
                if secret(key) && item == MASK {
                    *item = previous
                        .get(key)
                        .filter(|v| !v.is_null())
                        .cloned()
                        .ok_or_else(|| {
                            ApiError::BadRequest("新对象必须填写凭据，不能使用保留占位符".into())
                        })?;
                } else {
                    restore(item, &previous[key])?;
                }
            }
        }
        Value::Array(items) => {
            for (index, item) in items.iter_mut().enumerate() {
                // Stable tags/names prevent secret mix-ups when an editor reorders objects.
                let identity = item.get("tag").or_else(|| item.get("name"));
                let old = if let Some(identity) = identity {
                    previous.as_array().and_then(|v| {
                        v.iter().find(|old| {
                            old.get("tag").or_else(|| old.get("name")) == Some(identity)
                        })
                    })
                } else {
                    previous.get(index)
                };
                restore(item, old.unwrap_or(&Value::Null))?;
            }
        }
        _ => {}
    }
    Ok(())
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    expected_revision: i64,
    definition: Value,
}

pub async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> ApiResult<Json<Value>> {
    crate::control_center::require_server(&state, &headers, id, "proxy:read").await?;
    let row: Option<(Value, i64, i64)> = sqlx::query_as(
        "SELECT definition,revision,updated_at FROM singbox_server_profiles WHERE server_id=$1",
    )
    .bind(id)
    .fetch_optional(&state.pool)
    .await?;
    let (mut definition, revision, updated_at) =
        row.unwrap_or((json!(ServerProfile::default()), 0, 0));
    redact(&mut definition);
    Ok(Json(
        json!({"definition":definition,"revision":revision,"updated_at":updated_at}),
    ))
}
pub async fn put(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Json(mut request): Json<Request>,
) -> ApiResult<Json<Value>> {
    // The administration guard audits this request with secrets removed.
    crate::control_center::require_server(&state, &headers, id, "proxy:write").await?;
    crate::control_center::require_recent_proof(&state, &headers).await?;
    if !(0..business::MAX_SAFE_INTEGER).contains(&request.expected_revision) {
        return Err(ApiError::BadRequest("版本无效".into()));
    }
    let mut tx = state.pool.begin().await?;
    business::lock_publication(&mut tx).await?;
    crate::servers::lock(&mut tx, id).await?;
    let previous: Option<(Value, i64)> = sqlx::query_as(
        "SELECT definition,revision FROM singbox_server_profiles WHERE server_id=$1 FOR UPDATE",
    )
    .bind(id)
    .fetch_optional(&mut *tx)
    .await?;
    if previous.as_ref().map_or(0, |(_, revision)| *revision) != request.expected_revision {
        return Err(ApiError::Conflict("配置已变化，请刷新后重试".into()));
    }
    restore(
        &mut request.definition,
        &previous.map_or(Value::Null, |(v, _)| v),
    )?;
    let definition: ServerProfile = serde_json::from_value(request.definition.clone())
        .map_err(|_| ApiError::BadRequest("服务端配置结构无效".into()))?;
    definition
        .validate()
        .map_err(|e| ApiError::BadRequest(e.into()))?;
    let architecture: String =
        sqlx::query_scalar("SELECT COALESCE(static_info->>'system','') FROM servers WHERE id=$1")
            .bind(id)
            .fetch_one(&mut *tx)
            .await?;
    if architecture.eq_ignore_ascii_case("freebsd")
        && definition.outbounds.iter().any(|v| v["type"] == "naive")
    {
        return Err(ApiError::BadRequest(
            "FreeBSD 没有可用的 Cronet 构建，不支持 Naive 出站".into(),
        ));
    }
    sqlx::query("INSERT INTO singbox_server_profiles(server_id,definition,revision,updated_at) VALUES($1,$2,$3,$4) ON CONFLICT(server_id) DO UPDATE SET definition=EXCLUDED.definition,revision=EXCLUDED.revision,updated_at=EXCLUDED.updated_at")
      .bind(id).bind(&request.definition).bind(request.expected_revision+1).bind(apsp_protocol::now_timestamp()).execute(&mut *tx).await?;
    // Compile the complete candidate before committing; publication performs its usual preflight.
    crate::publisher::preview_profile(&state, &mut tx, id)
        .await
        .map_err(|_| {
            ApiError::BadRequest("配置与当前节点或链路不兼容，请检查对象引用和监听端口".into())
        })?;
    business::mark_dirty(&mut tx, &[id]).await?;
    tx.commit().await?;
    Ok(Json(json!({"revision":request.expected_revision+1})))
}
pub(crate) async fn compile(
    tx: &mut Transaction<'_, Postgres>,
    id: i64,
    content: &str,
) -> anyhow::Result<String> {
    let value: Option<Value> =
        sqlx::query_scalar("SELECT definition FROM singbox_server_profiles WHERE server_id=$1")
            .bind(id)
            .fetch_optional(&mut **tx)
            .await?;
    match value {
        Some(value) => serde_json::from_value::<ServerProfile>(value)?
            .apply(content)
            .map_err(anyhow::Error::msg),
        None => Ok(content.to_owned()),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn secrets_follow_identity_after_reordering() {
        let original = json!({"outbounds":[{"tag":"extra-a","password":"TEST_ONLY A"},{"tag":"extra-b","password":"TEST_ONLY B"}]});
        let mut edited = json!({"outbounds":[{"tag":"extra-b","password":MASK},{"tag":"extra-a","password":MASK}]});
        restore(&mut edited, &original).unwrap();
        assert_eq!(edited["outbounds"][0]["password"], "TEST_ONLY B");
        edited["mca_key"] = json!("TEST_ONLY auxiliary private key");
        redact(&mut edited);
        assert!(!edited.to_string().contains("TEST_ONLY"));
        let mut new = json!({"outbounds":[{"tag":"extra-c","password":MASK}]});
        assert!(restore(&mut new, &original).is_err());
    }
}
