use super::*;
use axum::{
    Json,
    extract::{Path, State},
    http::HeaderMap,
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Input {
    kind: String,
    title: String,
    category: String,
    body: String,
    published: bool,
    version: Option<i64>,
}
pub(super) async fn list(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> ApiResult<Json<Value>> {
    administrator(&state, &headers, false).await?;
    let rows:Vec<Value>=sqlx::query_scalar("SELECT to_jsonb(c) FROM singbox_member_content c ORDER BY updated_at DESC,id DESC LIMIT 500").fetch_all(&state.pool).await?;
    Ok(Json(json!({"entries":rows})))
}
pub(super) async fn create(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<Input>,
) -> ApiResult<Json<Value>> {
    let actor = administrator(&state, &headers, true).await?;
    Ok(Json(save(&state, &actor, None, input).await?))
}
pub(super) async fn update(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Json(input): Json<Input>,
) -> ApiResult<Json<Value>> {
    let actor = administrator(&state, &headers, true).await?;
    Ok(Json(save(&state, &actor, Some(id), input).await?))
}
async fn save(state: &AppState, actor: &str, id: Option<i64>, input: Input) -> ApiResult<Value> {
    if !["notice", "guide"].contains(&input.kind.as_str()) {
        return Err(bad("内容类型无效"));
    }
    let title = text(&input.title, 200, true)?;
    let category = text(&input.category, 100, false)?;
    let body = text(&input.body, 100_000, true)?;
    let mut tx = state.pool.begin().await?;
    let query = if id.is_some() {
        "UPDATE singbox_member_content SET kind=$1,title=$2,category=$3,body=$4,published=$5,updated_at=$6,version=version+1 WHERE id=$7 AND version=$8 RETURNING to_jsonb(singbox_member_content)"
    } else {
        "INSERT INTO singbox_member_content(kind,title,category,body,published,updated_at) SELECT $1,$2,$3,$4,$5,$6 WHERE $7::bigint IS NULL AND $8::bigint IS NULL RETURNING to_jsonb(singbox_member_content)"
    };
    let value: Value = sqlx::query_scalar(query)
        .bind(input.kind)
        .bind(title)
        .bind(category)
        .bind(body)
        .bind(input.published)
        .bind(now_timestamp())
        .bind(id)
        .bind(input.version)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| conflict("内容已变化，请刷新后保存"))?;
    event(
        &mut tx,
        None,
        actor,
        "content_saved",
        &value["id"].to_string(),
        json!({"published":input.published}),
    )
    .await?;
    tx.commit().await?;
    Ok(value)
}

// Protected guide regions are removed before the member response is serialized.
pub(super) fn render(body: &str, allowed: bool, url: &str) -> String {
    use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
    let mut body = body.to_owned();
    if !allowed {
        while let Some(start) = body.find("<!--access start-->") {
            let end = body[start..]
                .find("<!--access end-->")
                .map(|end| start + end + "<!--access end-->".len())
                .unwrap_or(body.len());
            body.replace_range(start..end, "<p>有效套餐开通后可查看此部分内容。</p>");
        }
    }
    body.replace("{{siteName}}", "apsp")
        .replace("{{subscribeUrl}}", url)
        .replace(
            "{{urlEncodeSubscribeUrl}}",
            &url.bytes()
                .map(|b| {
                    if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
                        char::from(b).to_string()
                    } else {
                        format!("%{b:02X}")
                    }
                })
                .collect::<String>(),
        )
        .replace(
            "{{safeBase64SubscribeUrl}}",
            &URL_SAFE_NO_PAD.encode(url.as_bytes()),
        )
}
#[cfg(test)]
mod tests {
    use super::render;
    #[test]
    fn protected_regions_are_removed_and_subscription_placeholders_are_scoped() {
        let body = "Public<!--access start-->private<!--access end-->{{subscribeUrl}}";
        let rendered = render(body, false, "https://legacy.example.com/assets/fixture");
        assert!(!rendered.contains("private"));
        assert!(rendered.contains("/assets/fixture"));
        assert!(render(body, true, "fixture").contains("private"));
        assert!(
            !render("<!--access start-->unclosed private", false, "fixture").contains("private")
        );
        assert_eq!(
            render(
                "{{siteName}} {{urlEncodeSubscribeUrl}} {{safeBase64SubscribeUrl}}",
                true,
                "a:b"
            ),
            "apsp a%3Ab YTpi"
        );
    }
}
