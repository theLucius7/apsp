#![forbid(unsafe_code)]
mod business_support;
#[path = "../../protocol/tests/support/release.rs"]
mod release_support;
use anyhow::{Context, Result, ensure};
use apsp_panel::auth::hash_token;
use apsp_protocol::now_timestamp;
use business_support::TestPanel;
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;
const ROOT: &str = "/api/plugins/expenses";
async fn call(
    panel: &TestPanel,
    cookie: &str,
    method: Method,
    path: &str,
    value: Value,
) -> Result<reqwest::Response> {
    panel
        .admin(method, &format!("{ROOT}{path}"), cookie, Some(value))
        .await
}
async fn ok(response: reqwest::Response) -> Result<Value> {
    let status = response.status();
    let body = response.text().await?;
    ensure!(status.is_success(), "{status}: {body}");
    Ok(serde_json::from_str(&body)?)
}
fn input() -> Value {
    json!({"name":"Fixture subscription","amount_units":"123456","currency":"USD","start_date":"2026-01-31","next_payment":"2026-01-31","cycle":"month","frequency":1,"category_id":null,"payment_method_id":null,"payer_id":null,"notes":"A, B; C\nD","url":"https://example.com","inactive":false,"auto_renew":true,"notify":true,"notify_days_before":3,"cancellation_date":null,"replacement_id":null,"version":null})
}
#[sqlx::test(migrations = "./migrations")]
async fn expenses_preserve_dates_exact_amounts_versions_and_import_identity(
    pool: PgPool,
) -> Result<()> {
    let panel = TestPanel::start(pool.clone()).await?;
    let cookie = panel.admin_cookie().await?;
    let hash = hash_token(cookie.split_once('=').context("cookie")?.1);
    sqlx::query("INSERT INTO administrator_reauth(session_hash,verified_at,expires_at) VALUES($1,$2,$3) ON CONFLICT DO NOTHING").bind(hash).bind(now_timestamp()).bind(now_timestamp()+3600).execute(&pool).await?;
    assert_eq!(
        panel
            .client
            .get(format!("{}{ROOT}/subscriptions", panel.base))
            .send()
            .await?
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let label = ok(call(
        &panel,
        &cookie,
        Method::POST,
        "/labels",
        json!({"kind":"category","name":"Network","enabled":true}),
    )
    .await?)
    .await?;
    let mut value = input();
    value["category_id"] = label["id"].clone();
    let entry = ok(call(
        &panel,
        &cookie,
        Method::POST,
        "/subscriptions",
        value.clone(),
    )
    .await?)
    .await?;
    let id = entry["id"].as_i64().context("id")?;
    assert_eq!(entry["amount_units"], "123456");
    let dates = ok(call(
        &panel,
        &cookie,
        Method::GET,
        "/calendar?from=2026-01-01&to=2026-03-31",
        Value::Null,
    )
    .await?)
    .await?;
    let dates: Vec<_> = dates["entries"]
        .as_array()
        .context("dates")?
        .iter()
        .map(|v| v["date"].as_str().unwrap())
        .collect();
    assert_eq!(dates, vec!["2026-01-31", "2026-02-28", "2026-03-31"]);
    ok(call(
        &panel,
        &cookie,
        Method::PUT,
        &format!("/labels/{}", label["id"]),
        json!({"kind":"category","name":"Network","enabled":false}),
    )
    .await?)
    .await?;
    value["version"] = json!(1);
    value["notes"] = json!("changed");
    let updated = ok(call(
        &panel,
        &cookie,
        Method::PUT,
        &format!("/subscriptions/{id}"),
        value.clone(),
    )
    .await?)
    .await?;
    assert_eq!(updated["version"], 2);
    assert_eq!(
        call(
            &panel,
            &cookie,
            Method::PUT,
            &format!("/subscriptions/{id}"),
            value
        )
        .await?
        .status(),
        StatusCode::CONFLICT
    );
    let operation = json!({"request_id":Uuid::new_v4(),"version":2});
    let first = ok(call(
        &panel,
        &cookie,
        Method::POST,
        &format!("/subscriptions/{id}/renew"),
        operation.clone(),
    )
    .await?)
    .await?;
    let again = ok(call(
        &panel,
        &cookie,
        Method::POST,
        &format!("/subscriptions/{id}/renew"),
        operation,
    )
    .await?)
    .await?;
    assert_eq!(first, again);
    let export = ok(call(&panel, &cookie, Method::GET, "/export", Value::Null).await?).await?;
    let preview = ok(call(
        &panel,
        &cookie,
        Method::POST,
        "/import/preview",
        export.clone(),
    )
    .await?)
    .await?;
    let load = json!({"snapshot":export,"confirmed_digest":preview["digest"]});
    let receipt = ok(call(&panel, &cookie, Method::POST, "/import", load.clone()).await?).await?;
    assert_eq!(
        receipt,
        ok(call(&panel, &cookie, Method::POST, "/import", load).await?).await?
    );
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM expense_subscriptions")
        .fetch_one(&pool)
        .await?;
    assert_eq!(count, 2);
    let reminders: bool =
        sqlx::query_scalar("SELECT reminders_enabled FROM expense_settings WHERE singleton")
            .fetch_one(&pool)
            .await?;
    assert!(!reminders);
    let mut dangerous = input();
    dangerous["url"] = json!("javascript:alert(1)");
    assert_eq!(
        call(&panel, &cookie, Method::POST, "/subscriptions", dangerous)
            .await?
            .status(),
        StatusCode::BAD_REQUEST
    );
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn servers_join_the_ledger_read_only_and_renewals_record_payments(
    pool: PgPool,
) -> Result<()> {
    let panel = TestPanel::start(pool.clone()).await?;
    let cookie = panel.admin_cookie().await?;
    let expires = now_timestamp() + 10 * 86400;
    for (name, asset) in [
        (
            "priced",
            json!({"price":"12.5","currency":"usd","billing_cycle":30,"expires_at":expires,"traffic_limit":"0","reset_day":1}),
        ),
        (
            "unpriced",
            json!({"currency":"CNY","billing_cycle":0,"expires_at":expires,"traffic_limit":"0","reset_day":1}),
        ),
        (
            "no-expiry",
            json!({"price":"5","currency":"CNY","billing_cycle":30,"traffic_limit":"0","reset_day":1}),
        ),
    ] {
        let response = panel
            .admin(
                Method::POST,
                "/api/servers",
                &cookie,
                Some(json!({"name":name,"asset_settings":asset})),
            )
            .await?;
        ensure!(
            response.status() == StatusCode::CREATED,
            "{name}: {}",
            response.text().await?
        );
    }
    let servers = ok(call(&panel, &cookie, Method::GET, "/servers", Value::Null).await?).await?;
    let entries = servers["entries"].as_array().cloned().unwrap_or_default();
    ensure!(
        entries.len() == 2,
        "servers without an expiry stay out: {servers}"
    );
    ensure!(entries[0]["amount_units"] == "125000" && entries[0]["currency"] == "USD");
    ensure!(entries[1]["amount_units"].is_null());

    let summary = ok(call(&panel, &cookie, Method::GET, "/summary", Value::Null).await?).await?;
    ensure!(
        summary["servers"]["count"] == 2 && summary["servers"]["unpriced"] == 1,
        "{summary}"
    );
    // 12.5 USD every 30 days from day 10: days 10, 40, ..., 340 are 12 renewals.
    ensure!(
        summary["servers"]["totals_units"]["USD"] == "1500000",
        "{summary}"
    );
    ensure!(summary["totals_units"]["USD"] == "1500000", "{summary}");
    let upcoming = summary["upcoming"].as_array().cloned().unwrap_or_default();
    ensure!(
        upcoming.iter().filter(|e| e["kind"] == "server").count() == 2,
        "both servers renew within 30 days: {summary}"
    );

    let mut value = input();
    value["next_payment"] = json!("2099-01-31");
    value["start_date"] = Value::Null;
    let created = ok(call(&panel, &cookie, Method::POST, "/subscriptions", value).await?).await?;
    let id = created["id"].as_i64().context("id")?;
    let renew = json!({"request_id":Uuid::new_v4(),"version":created["version"]});
    for _ in 0..2 {
        ok(call(
            &panel,
            &cookie,
            Method::POST,
            &format!("/subscriptions/{id}/renew"),
            renew.clone(),
        )
        .await?)
        .await?;
    }
    let payments = ok(call(
        &panel,
        &cookie,
        Method::GET,
        &format!("/subscriptions/{id}/payments"),
        Value::Null,
    )
    .await?)
    .await?;
    let entries = payments["entries"].as_array().cloned().unwrap_or_default();
    ensure!(
        entries.len() == 1,
        "a replayed renewal records one payment: {payments}"
    );
    ensure!(
        entries[0]["period_start"] == "2099-01-31" && entries[0]["amount_units"] == "123456",
        "{payments}"
    );
    let calendar = panel
        .admin(
            Method::GET,
            &format!("{ROOT}/calendar.ics?from=2099-02-01&to=2099-03-31"),
            &cookie,
            None,
        )
        .await?;
    ensure!(calendar.status().is_success(), "{}", calendar.status());
    let calendar = calendar.text().await?;
    ensure!(
        calendar.contains(&format!("UID:expense-{id}-20990228@apsp.invalid")),
        "item UIDs keep their published form: {calendar}"
    );
    ensure!(calendar.contains("UID:expense-server-"), "{calendar}");
    // The ledger never writes server data.
    let unchanged: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM servers WHERE (asset_settings->>'expires_at')::bigint=$1",
    )
    .bind(expires)
    .fetch_one(&pool)
    .await?;
    ensure!(unchanged == 2);
    Ok(())
}
