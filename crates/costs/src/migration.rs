use super::*;
use axum::{Json, extract::State};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Snapshot {
    format: String,
    source_id: String,
    base_currency: String,
    budget_units: Option<String>,
    timezone: String,
    labels: Vec<Label>,
    subscriptions: Vec<Entry>,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Label {
    id: i64,
    kind: String,
    name: String,
    enabled: bool,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    id: i64,
    anchor_payment: String,
    configuration: Input,
}
fn validate(snapshot: &Snapshot) -> ApiResult<String> {
    if snapshot.format != "apsp-expenses-v1"
        || snapshot.labels.len() > 5000
        || snapshot.subscriptions.len() > 10000
        || !currency(&snapshot.base_currency)
    {
        return Err(bad("导入格式、币种或条目数量无效"));
    }
    text(&snapshot.source_id, 200, true)?;
    if let Some(value) = &snapshot.budget_units {
        units(value)?;
    }
    let mut labels = BTreeMap::new();
    let mut ids = BTreeSet::new();
    for label in &snapshot.labels {
        if label.id <= 0
            || labels.insert(label.id, label.kind.as_str()).is_some()
            || !["category", "payment_method", "payer"].contains(&label.kind.as_str())
        {
            return Err(bad("分类编号或类型无效"));
        }
        text(&label.name, 100, true)?;
    }
    for entry in &snapshot.subscriptions {
        if entry.id <= 0 || !ids.insert(entry.id) {
            return Err(bad("订阅编号无效或重复"));
        }
        validate_input(entry, &labels)?;
    }
    let replacements: BTreeMap<_, _> = snapshot
        .subscriptions
        .iter()
        .filter_map(|s| s.configuration.replacement_id.map(|v| (s.id, v)))
        .collect();
    for entry in &snapshot.subscriptions {
        let mut visited = BTreeSet::from([entry.id]);
        let mut current = entry.id;
        while let Some(next) = replacements.get(&current) {
            if !ids.contains(next) || !visited.insert(*next) {
                return Err(bad("替代订阅缺失或形成循环"));
            }
            current = *next;
        }
    }
    let bytes = serde_json::to_vec(snapshot).map_err(anyhow::Error::from)?;
    if bytes.len() > 8 * 1024 * 1024 {
        return Err(bad("导入文件超过 8 MiB"));
    }
    Ok(format!("{:x}", Sha256::digest(bytes)))
}
fn validate_input(entry: &Entry, labels: &BTreeMap<i64, &str>) -> ApiResult<()> {
    super::validate(&entry.configuration)?;
    let anchor = schedule::parse(&entry.anchor_payment)?;
    let next = schedule::parse(&entry.configuration.next_payment)?;
    if schedule::at_or_after(
        anchor,
        &entry.configuration.cycle,
        entry.configuration.frequency,
        next,
    )
    .map(|x| x.0)
        != Some(next)
    {
        return Err(bad("下次付款与原始周期不一致"));
    }
    if entry.configuration.version.is_some() {
        return Err(bad("导入条目不能带有目标版本号"));
    }
    for (id, kind) in [
        (entry.configuration.category_id, "category"),
        (entry.configuration.payment_method_id, "payment_method"),
        (entry.configuration.payer_id, "payer"),
    ] {
        if let Some(id) = id
            && labels.get(&id) != Some(&kind)
        {
            return Err(bad("导入条目的分类、付款方式或付款人缺失"));
        }
    }
    Ok(())
}
async fn preview_on(tx: &mut Tx<'_>, snapshot: &Snapshot, digest: &str) -> ApiResult<Value> {
    let zone: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_timezone_names WHERE name=$1)")
            .bind(&snapshot.timezone)
            .fetch_one(&mut **tx)
            .await?;
    if !zone {
        return Err(bad("导入时区无效"));
    }
    let previous: Option<(String, Value)> =
        sqlx::query_as("SELECT digest,receipt FROM expense_imports WHERE source_id=$1")
            .bind(&snapshot.source_id)
            .fetch_optional(&mut **tx)
            .await?;
    if let Some((previous, receipt)) = previous {
        if previous != digest {
            return Err(conflict("该来源已经导入过不同内容，请使用新的来源编号"));
        }
        return Ok(json!({"digest":digest,"already_imported":true,"receipt":receipt}));
    }
    Ok(
        json!({"digest":digest,"already_imported":false,"subscriptions":snapshot.subscriptions.len(),"labels":snapshot.labels.len(),"warnings":["仅新增配置，不覆盖已有订阅；同名同类标签会复用。","全局提醒保持关闭；导入后核对时区、日期与币种，再自行开启。","基础币种、预算与时区仅在目标没有订阅时采用；不会迁移密码或通知密钥。"]}),
    )
}
pub(super) async fn preview(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(snapshot): Json<Snapshot>,
) -> ApiResult<Json<Value>> {
    actor(&state, &headers, true).await?;
    let digest = validate(&snapshot)?;
    let mut tx = state.pool.begin().await?;
    Ok(Json(preview_on(&mut tx, &snapshot, &digest).await?))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Import {
    snapshot: Snapshot,
    confirmed_digest: String,
}
pub(super) async fn import(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<Import>,
) -> ApiResult<Json<Value>> {
    let administrator = actor(&state, &headers, true).await?;
    control_center::require_recent_proof(&state, &headers).await?;
    let snapshot = input.snapshot;
    let digest = validate(&snapshot)?;
    if digest != input.confirmed_digest {
        return Err(conflict("文件与预览摘要不同，请重新预览"));
    }
    let mut tx = state.pool.begin().await?;
    lock(&mut tx).await?;
    let preview = preview_on(&mut tx, &snapshot, &digest).await?;
    if preview["already_imported"] == true {
        return Ok(Json(preview["receipt"].clone()));
    }
    let empty: bool = sqlx::query_scalar("SELECT NOT EXISTS(SELECT 1 FROM expense_subscriptions)")
        .fetch_one(&mut *tx)
        .await?;
    let mut labels = BTreeMap::new();
    for label in &snapshot.labels {
        let id:i64=sqlx::query_scalar("INSERT INTO expense_labels(kind,name,enabled) VALUES($1,$2,$3) ON CONFLICT(kind,name) DO UPDATE SET name=EXCLUDED.name RETURNING id").bind(&label.kind).bind(label.name.trim()).bind(label.enabled).fetch_one(&mut *tx).await?;
        labels.insert(label.id, id);
    }
    let mut entries = BTreeMap::new();
    for entry in &snapshot.subscriptions {
        let mut input = entry.configuration.clone();
        input.category_id = input.category_id.map(|i| labels[&i]);
        input.payment_method_id = input.payment_method_id.map(|i| labels[&i]);
        input.payer_id = input.payer_id.map(|i| labels[&i]);
        input.replacement_id = None;
        // Disabled labels remain selectable for restoring historical configuration.
        let value = save_imported(&mut tx, &input).await?;
        sqlx::query("UPDATE expense_subscriptions SET anchor_payment=$2::text::date WHERE id=$1")
            .bind(value.id)
            .bind(&entry.anchor_payment)
            .execute(&mut *tx)
            .await?;
        entries.insert(entry.id, value.id);
    }
    for entry in &snapshot.subscriptions {
        if let Some(replacement) = entry.configuration.replacement_id {
            sqlx::query("UPDATE expense_subscriptions SET replacement_id=$2 WHERE id=$1")
                .bind(entries[&entry.id])
                .bind(entries[&replacement])
                .execute(&mut *tx)
                .await?;
        }
    }
    if empty {
        sqlx::query("UPDATE expense_settings SET base_currency=$1,budget_units=$2,timezone=$3,version=version+1 WHERE singleton").bind(&snapshot.base_currency).bind(snapshot.budget_units.as_deref().map(units).transpose()?).bind(&snapshot.timezone).execute(&mut *tx).await?;
    }
    sqlx::query(
        "UPDATE expense_settings SET reminders_enabled=FALSE,version=version+1 WHERE singleton",
    )
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "UPDATE expense_deliveries SET state='cancelled' WHERE state IN('pending','failed')",
    )
    .execute(&mut *tx)
    .await?;
    let receipt = json!({"source_id":snapshot.source_id,"digest":digest,"subscriptions":entries.len(),"labels":labels.len(),"id_mapping":entries,"reminders_enabled":false});
    sqlx::query("INSERT INTO expense_imports(id,source_id,digest,receipt,created_at) VALUES($1,$2,$3,$4,$5)").bind(Uuid::new_v4()).bind(&snapshot.source_id).bind(digest).bind(&receipt).bind(now_timestamp()).execute(&mut *tx).await?;
    event(&mut tx,None,"imported",json!({"administrator_id":administrator,"source_id":snapshot.source_id,"subscriptions":entries.len()})).await?;
    tx.commit().await?;
    Ok(Json(receipt))
}
async fn save_imported(tx: &mut Tx<'_>, input: &Input) -> ApiResult<Subscription> {
    // Enable referenced labels inside this transaction only, then restore their states.
    let ids: Vec<i64> = [input.category_id, input.payment_method_id, input.payer_id]
        .into_iter()
        .flatten()
        .collect();
    let disabled: Vec<i64> = sqlx::query_scalar(
        "UPDATE expense_labels SET enabled=TRUE WHERE id=ANY($1) AND NOT enabled RETURNING id",
    )
    .bind(&ids)
    .fetch_all(&mut **tx)
    .await?;
    let value = save(tx, None, input).await?;
    sqlx::query("UPDATE expense_labels SET enabled=FALSE WHERE id=ANY($1)")
        .bind(&disabled)
        .execute(&mut **tx)
        .await?;
    Ok(value)
}
pub(super) async fn export(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> ApiResult<axum::response::Response> {
    use axum::{http::header, response::IntoResponse};
    actor(&state, &headers, false).await?;
    let mut tx = state.pool.begin().await?;
    lock(&mut tx).await?;
    let config = sqlx::query(
        "SELECT base_currency,budget_units::text,timezone FROM expense_settings WHERE singleton",
    )
    .fetch_one(&mut *tx)
    .await?;
    let labels: Vec<Value> =
        sqlx::query_scalar("SELECT to_jsonb(l) FROM expense_labels l ORDER BY id")
            .fetch_all(&mut *tx)
            .await?;
    let entries: Vec<Subscription> = sqlx::query_as(&format!(
        "SELECT {COLUMNS} FROM expense_subscriptions ORDER BY id"
    ))
    .fetch_all(&mut *tx)
    .await?;
    let mut subscriptions = Vec::new();
    for entry in entries {
        let mut value = serde_json::to_value(&entry).map_err(anyhow::Error::from)?;
        let object = value.as_object_mut().ok_or_else(|| bad("导出格式错误"))?;
        object.remove("id");
        object.remove("anchor_payment");
        object.insert("version".into(), Value::Null);
        subscriptions.push(
            json!({"id":entry.id,"anchor_payment":entry.anchor_payment,"configuration":value}),
        );
    }
    let value = json!({"format":"apsp-expenses-v1","source_id":format!("apsp-export-{}",Uuid::new_v4()),"base_currency":config.try_get::<String,_>("base_currency")?,"budget_units":config.try_get::<Option<String>,_>("budget_units")?,"timezone":config.try_get::<String,_>("timezone")?,"labels":labels,"subscriptions":subscriptions});
    tx.commit().await?;
    let mut response = Json(value).into_response();
    response.headers_mut().insert(
        header::CONTENT_DISPOSITION,
        header::HeaderValue::from_static("attachment; filename=apsp-expenses.json"),
    );
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("no-store"),
    );
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn strict_amounts_and_cycles() {
        assert!(units("1.00").is_err());
        assert!(units("-1").is_err());
        assert!(units("9000000000000001").is_err());
        assert_eq!(units("12345").unwrap(), 12345);
        assert!(currency("CNY"));
        assert!(!currency("cny"));
    }
}
