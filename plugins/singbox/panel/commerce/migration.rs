use super::*;
use axum::{
    Json,
    extract::{Query, State},
    http::HeaderMap,
};
use std::collections::BTreeSet;

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Snapshot {
    pub format: String,
    pub source_id: String,
    pub exported_at: i64,
    pub members: Vec<Member>,
    pub plans: Vec<Plan>,
    pub history: Vec<History>,
    pub content: Vec<Content>,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Member {
    pub id: i64,
    pub name: String,
    pub email: String,
    pub balance_cents: i64,
    pub state: String,
    pub inviter_id: Option<i64>,
    pub sponsor_required: bool,
    pub first_purchase_at: Option<i64>,
    pub subscription_token: String,
    pub legacy_uuid: Uuid,
    pub plan_id: Option<i64>,
    pub quota_bytes: Option<String>,
    pub used_upload: String,
    pub used_download: String,
    pub expires_at: Option<i64>,
    pub reset_at: Option<i64>,
    #[serde(default)]
    pub legacy_password_hash: Option<String>,
    #[serde(default)]
    pub last_reset_at: Option<i64>,
    #[serde(default = "default_reminder")]
    pub remind_expire: bool,
    #[serde(default = "default_reminder")]
    pub remind_traffic: bool,
}
fn default_reminder() -> bool {
    true
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Plan {
    pub id: i64,
    pub name: String,
    pub quota_bytes: Option<String>,
    pub prices: Vec<Price>,
    #[serde(default)]
    pub legacy_group_id: Option<i64>,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Price {
    pub label: String,
    pub cents: i64,
    pub months: Option<i32>,
    pub days: i32,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct History {
    pub kind: String,
    pub id: String,
    pub member_id: Option<i64>,
    pub record: Value,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Content {
    pub id: i64,
    pub kind: String,
    pub title: String,
    pub category: String,
    pub body: String,
    pub published: bool,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Import {
    snapshot: Snapshot,
    expected_digest: String,
    confirm_staged: bool,
}

fn canonical_bytes(value: &str) -> bool {
    value.parse::<u64>().is_ok_and(|n| n.to_string() == value)
}
fn private_record(value: &Value) -> bool {
    match value {
        Value::Object(values) => values.iter().any(|(key, value)| {
            let key = key.to_lowercase();
            [
                "password",
                "secret",
                "token",
                "credential",
                "private_key",
                "email",
                "uuid",
                "config",
            ]
            .iter()
            .any(|s| key.contains(s))
                || private_record(value)
        }),
        Value::Array(values) => values.iter().any(private_record),
        _ => false,
    }
}
pub fn validate(snapshot: &Snapshot) -> ApiResult<Value> {
    if snapshot.format != "apsp.nexus.v1"
        || snapshot.source_id.is_empty()
        || snapshot.source_id.len() > 128
        || snapshot.source_id.chars().any(char::is_control)
        || snapshot.members.len() > 10_000
        || snapshot.plans.len() > 1000
        || snapshot.history.len() > 100_000
        || snapshot.content.len() > 1000
    {
        return Err(bad("迁移文件格式或数量超出范围"));
    }
    // A future export time would hold the legacy meter's observations frozen.
    if snapshot.exported_at <= 0 || snapshot.exported_at > now_timestamp() + 300 {
        return Err(bad("导出时间无效或晚于当前时间"));
    }
    let mut ids = BTreeSet::new();
    let mut legacy_uuids = BTreeSet::new();
    let mut emails = BTreeSet::new();
    let mut tokens = BTreeSet::new();
    let mut plans = BTreeSet::new();
    for plan in &snapshot.plans {
        if plan.id <= 0
            || !plans.insert(plan.id)
            || plan.prices.len() > 20
            || plan
                .quota_bytes
                .as_ref()
                .is_some_and(|v| !canonical_bytes(v) || v == "0")
        {
            return Err(bad("原套餐编号或流量额度无效"));
        }
        crate::business::name(&plan.name)?;
        for price in &plan.prices {
            text(&price.label, 100, true)?;
            if !(0..=100_000_000).contains(&price.cents)
                || !(1..=36500).contains(&price.days)
                || price.months.is_some_and(|n| !(1..=1200).contains(&n))
            {
                return Err(bad("原套餐价格或周期无效"));
            }
        }
    }
    for m in &snapshot.members {
        if m.id <= 0
            || m.id > crate::business::MAX_SAFE_INTEGER
            || !ids.insert(m.id)
            || !emails.insert(email(&m.email)?)
            || !(0..=MAX_BALANCE).contains(&m.balance_cents)
            || !["active", "pending", "suspended", "archived"].contains(&m.state.as_str())
            || m.subscription_token.len() < 16
            || m.subscription_token.len() > 128
            || !m
                .subscription_token
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
            || !tokens.insert(&m.subscription_token)
            // Two members with one legacy UUID would share node credentials.
            || !legacy_uuids.insert(m.legacy_uuid)
            || !canonical_bytes(&m.used_upload)
            || !canonical_bytes(&m.used_download)
            || m.quota_bytes.as_ref().is_some_and(|v| !canonical_bytes(v))
            || m.plan_id.is_some_and(|id| !plans.contains(&id))
        {
            return Err(bad("成员编号、邮箱、余额、令牌或套餐关系无效"));
        }
        crate::business::name(&m.name)?;
        if let Some(hash) = &m.legacy_password_hash {
            let cost = hash.get(4..6).and_then(|v| v.parse::<u8>().ok());
            if hash.len() != 60
                || !["$2y$", "$2b$", "$2a$"].iter().any(|p| hash.starts_with(p))
                || !cost.is_some_and(|c| (4..=14).contains(&c))
                || !hash
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"$./".contains(&b))
            {
                return Err(bad("原密码哈希格式或成本参数不受支持"));
            }
        }
    }
    for m in &snapshot.members {
        if m.inviter_id
            .is_some_and(|id| id == m.id || !ids.contains(&id))
        {
            return Err(bad("邀请人引用不存在或引用自身"));
        }
    }
    let mut rows = BTreeSet::new();
    for record in &snapshot.history {
        if ![
            "order",
            "wallet",
            "invitation",
            "traffic",
            "payment",
            "reset",
        ]
        .contains(&record.kind.as_str())
            || record.id.len() > 128
            || record.id.is_empty()
            || !rows.insert((&record.kind, &record.id))
            || record.member_id.is_some_and(|id| !ids.contains(&id))
            || private_record(&record.record)
            || serde_json::to_vec(&record.record)
                .map_err(anyhow::Error::from)?
                .len()
                > 32768
        {
            return Err(bad("历史记录存在重复、无效引用或不允许导入的私密字段"));
        }
    }
    let mut content = BTreeSet::new();
    for c in &snapshot.content {
        if c.id <= 0
            || !["guide", "notice"].contains(&c.kind.as_str())
            || !content.insert((&c.kind, c.id))
        {
            return Err(bad("公告指南编号无效或重复"));
        }
        text(&c.title, 200, true)?;
        text(&c.category, 100, false)?;
        text(&c.body, 100_000, true)?;
    }
    let sum: i64 = snapshot.members.iter().map(|m| m.balance_cents).sum();
    Ok(
        json!({"members":snapshot.members.len(),"plans":snapshot.plans.len(),"history":snapshot.history.len(),"content":snapshot.content.len(),"opening_balance_cents":sum.to_string()}),
    )
}
async fn conflicts(tx: &mut Tx<'_>, snapshot: &Snapshot) -> ApiResult<Vec<String>> {
    let ids: Vec<i64> = snapshot.members.iter().map(|m| m.id).collect();
    let tokens: Vec<&str> = snapshot
        .members
        .iter()
        .map(|m| m.subscription_token.as_str())
        .collect();
    let emails: Vec<String> = snapshot
        .members
        .iter()
        .map(|m| email(&m.email))
        .collect::<ApiResult<_>>()?;
    let users: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM users WHERE id=ANY($1) OR subscription_token=ANY($2)",
    )
    .bind(ids)
    .bind(tokens)
    .fetch_one(&mut **tx)
    .await?;
    let members: i64 =
        sqlx::query_scalar("SELECT count(*) FROM singbox_members WHERE email=ANY($1)")
            .bind(emails)
            .fetch_one(&mut **tx)
            .await?;
    let mut errors = Vec::new();
    if users > 0 {
        errors.push(format!(
            "{users} 个原成员编号或订阅令牌与目标库冲突，拒绝覆盖"
        ));
    }
    if members > 0 {
        errors.push(format!("{members} 个联系邮箱与现有成员冲突"));
    }
    Ok(errors)
}
pub(super) async fn preview(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(snapshot): Json<Snapshot>,
) -> ApiResult<Json<Value>> {
    administrator(&state, &headers, true).await?;
    let counts = validate(&snapshot)?;
    let digest = fingerprint(&snapshot)?;
    let mut tx = state.pool.begin().await?;
    let existing: Option<String> =
        sqlx::query_scalar("SELECT digest FROM singbox_nexus_imports WHERE source_id=$1")
            .bind(&snapshot.source_id)
            .fetch_optional(&mut *tx)
            .await?;
    let errors = if existing.as_deref() == Some(&digest) {
        Vec::new()
    } else {
        conflicts(&mut tx, &snapshot).await?
    };
    Ok(Json(
        json!({"digest":digest,"counts":counts,"already_imported":existing.as_deref()==Some(&digest),"conflicts":errors,"cutover_ready":false,"remaining":["核对节点与策略授权映射","核对原密码登录或重新开通 Passkey","核对计费周期及旧客户端订阅格式","处理原系统未决支付和旧邀请链接","独立验证并切换唯一发布器"]}),
    ))
}
pub(super) async fn import(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<Import>,
) -> ApiResult<Json<Value>> {
    let actor = administrator(&state, &headers, true).await?;
    let counts = validate(&input.snapshot)?;
    let digest = fingerprint(&input.snapshot)?;
    if !input.confirm_staged || digest != input.expected_digest {
        return Err(conflict("请先预览同一份文件，并确认导入为待迁移成员"));
    }
    let snapshot = input.snapshot;
    let mut tx = state.pool.begin().await?;
    crate::entitlements::lock(&mut tx).await?;
    // Imported IDs must not race normal proxy-user creation.
    sqlx::query("LOCK TABLE users IN SHARE ROW EXCLUSIVE MODE")
        .execute(&mut *tx)
        .await?;
    if let Some(row) =
        sqlx::query("SELECT id,digest,counts FROM singbox_nexus_imports WHERE source_id=$1")
            .bind(&snapshot.source_id)
            .fetch_optional(&mut *tx)
            .await?
    {
        if row.try_get::<String, _>("digest")? != digest {
            return Err(conflict("同一来源已导入不同快照，拒绝覆盖"));
        }
        return Ok(Json(
            json!({"id":row.try_get::<Uuid,_>("id")?,"counts":row.try_get::<Value,_>("counts")?,"already_imported":true}),
        ));
    }
    let errors = conflicts(&mut tx, &snapshot).await?;
    if !errors.is_empty() {
        return Err(conflict(&errors.join("；")));
    }
    let import_id = Uuid::new_v4();
    let admin = actor
        .trim_start_matches("admin:")
        .parse::<i64>()
        .map_err(anyhow::Error::from)?;
    sqlx::query("INSERT INTO singbox_nexus_imports(id,source_id,digest,counts,imported_at,administrator_id) VALUES($1,$2,$3,$4,$5,$6)").bind(import_id).bind(&snapshot.source_id).bind(&digest).bind(&counts).bind(now_timestamp()).bind(admin).execute(&mut *tx).await?;
    for m in &snapshot.members {
        sqlx::query("INSERT INTO users(id,name,subscription_token) VALUES($1,$2,$3)")
            .bind(m.id)
            .bind(&m.name)
            .bind(&m.subscription_token)
            .execute(&mut *tx)
            .await?;
        sqlx::query("INSERT INTO singbox_members(user_id,email,state,sponsor_required,first_purchase_at,created_at) VALUES($1,$2,'staged',$3,$4,$5)").bind(m.id).bind(email(&m.email)?).bind(m.sponsor_required).bind(m.first_purchase_at).bind(now_timestamp()).execute(&mut *tx).await?;
        sqlx::query(
            "UPDATE singbox_members SET remind_expire=$2,remind_traffic=$3 WHERE user_id=$1",
        )
        .bind(m.id)
        .bind(m.remind_expire)
        .bind(m.remind_traffic)
        .execute(&mut *tx)
        .await?;
        if let Some(hash) = &m.legacy_password_hash {
            // The original login address stays usable for password login until changed.
            sqlx::query(
                "UPDATE singbox_members SET password_hash=$2,legacy_login_email=email WHERE user_id=$1",
            )
            .bind(m.id)
            .bind(hash)
            .execute(&mut *tx)
            .await?;
        }
        sqlx::query("INSERT INTO singbox_nexus_identities(user_id,import_id,legacy_uuid,original_state,original_plan_id,quota_bytes,used_upload,used_download,expires_at,reset_at) VALUES($1,$2,$3,$4,$5,$6::text::numeric,$7::text::numeric,$8::text::numeric,$9,$10)").bind(m.id).bind(import_id).bind(m.legacy_uuid).bind(&m.state).bind(m.plan_id).bind(&m.quota_bytes).bind(&m.used_upload).bind(&m.used_download).bind(m.expires_at).bind(m.reset_at).execute(&mut *tx).await?;
        sqlx::query("INSERT INTO singbox_nexus_metering(user_id,upload,download,last_reset_at,next_reset_at,observed_at) VALUES($1,$2::text::numeric,$3::text::numeric,$4,$5,$6)").bind(m.id).bind(&m.used_upload).bind(&m.used_download).bind(m.last_reset_at).bind(m.reset_at).bind(snapshot.exported_at).execute(&mut *tx).await?;
        wallet::post(
            &mut tx,
            wallet::Posting {
                user: m.id,
                key: &format!("nexus-opening:{import_id}"),
                action: "migration_opening",
                delta: m.balance_cents,
                source: &import_id.to_string(),
                actor: &actor,
                reason: "Nexus 已核对期初余额；历史流水不重复入账",
            },
        )
        .await?;
    }
    for m in &snapshot.members {
        sqlx::query("UPDATE singbox_members SET inviter_id=$2 WHERE user_id=$1")
            .bind(m.id)
            .bind(m.inviter_id)
            .execute(&mut *tx)
            .await?;
    }
    for plan in &snapshot.plans {
        sqlx::query("INSERT INTO singbox_nexus_records(import_id,kind,source_id,record) VALUES($1,'plan',$2,$3)").bind(import_id).bind(plan.id.to_string()).bind(serde_json::to_value(plan).map_err(anyhow::Error::from)?).execute(&mut *tx).await?;
        let package:i64=sqlx::query_scalar("INSERT INTO singbox_package_groups(name,monthly_bytes,reset_day,reset_hour,reset_minute,timezone,duration_days) VALUES($1,$2::text::numeric,1,0,0,'UTC',$3) RETURNING id").bind(&plan.name).bind(&plan.quota_bytes).bind(plan.prices.first().map(|p|p.days).unwrap_or(31)).fetch_one(&mut *tx).await?;
        sqlx::query("INSERT INTO singbox_nexus_plan_mappings(import_id,package_group_id,legacy_plan_id,legacy_group_id) VALUES($1,$2,$3,$4)").bind(import_id).bind(package).bind(plan.id).bind(plan.legacy_group_id).execute(&mut *tx).await?;
        for price in &plan.prices {
            let p = json!({"name":plan.name,"monthly_bytes":plan.quota_bytes,"reset_day":1,"reset_hour":0,"reset_minute":0,"timezone":"UTC","duration_days":price.days});
            sqlx::query("INSERT INTO singbox_commerce_offers(name,description,package_group_id,package_snapshot,price_cents,duration_months,enabled,created_at,updated_at) VALUES($1,$2,$3,$4,$5,$6,FALSE,$7,$7)").bind(format!("{} · {}",plan.name,price.label)).bind("从 Nexus 导入，核对周期和策略组后启用").bind(package).bind(p).bind(price.cents).bind(price.months).bind(now_timestamp()).execute(&mut *tx).await?;
        }
    }
    for r in &snapshot.history {
        sqlx::query("INSERT INTO singbox_nexus_records(import_id,kind,source_id,user_id,record) VALUES($1,$2,$3,$4,$5)").bind(import_id).bind(&r.kind).bind(&r.id).bind(r.member_id).bind(&r.record).execute(&mut *tx).await?;
    }
    for c in &snapshot.content {
        sqlx::query("INSERT INTO singbox_member_content(kind,title,category,body,published,updated_at) VALUES($1,$2,$3,$4,FALSE,$5)").bind(&c.kind).bind(&c.title).bind(&c.category).bind(&c.body).bind(now_timestamp()).execute(&mut *tx).await?;
    }
    sqlx::query("SELECT setval(pg_get_serial_sequence('users','id'),GREATEST(COALESCE((SELECT max(id) FROM users),1),(SELECT last_value FROM users_id_seq)),TRUE)").execute(&mut *tx).await?;
    event(
        &mut tx,
        None,
        &actor,
        "nexus_imported",
        &import_id.to_string(),
        counts.clone(),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(
        json!({"id":import_id,"counts":counts,"digest":digest,"cutover_ready":false}),
    ))
}
pub(super) async fn history(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> ApiResult<Json<Value>> {
    administrator(&state, &headers, false).await?;
    let rows: Vec<Value> = sqlx::query_scalar(
        "SELECT to_jsonb(i) FROM singbox_nexus_imports i ORDER BY imported_at DESC LIMIT 100",
    )
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(json!({"entries":rows})))
}

#[derive(Deserialize)]
pub(super) struct RecordFilter {
    kind: Option<String>,
    user_id: Option<i64>,
    #[serde(default)]
    offset: u32,
}
pub(super) async fn records(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(filter): Query<RecordFilter>,
) -> ApiResult<Json<Value>> {
    administrator(&state, &headers, false).await?;
    let entries:Vec<Value>=sqlx::query_scalar("SELECT to_jsonb(r)-'import_id' FROM singbox_nexus_records r WHERE ($1::text IS NULL OR kind=$1) AND ($2::bigint IS NULL OR user_id=$2) ORDER BY import_id,kind,source_id LIMIT 100 OFFSET $3").bind(filter.kind).bind(filter.user_id).bind(i64::from(filter.offset.min(1_000_000))).fetch_all(&state.pool).await?;
    Ok(Json(json!({"entries":entries})))
}
