use crate::cloud::billing;
use crate::error::ApiResult;
use apsp_cloud_api::{Failure, aliyun::Aliyun};
use apsp_resources::cloud::{
    account_on,
    client::Cloud,
    lock,
    model::{Account, Balance, CostRow, InstanceBill, Resource},
};
use serde_json::Value;
use sqlx::{PgPool, types::Json};
use std::collections::BTreeSet;
use uuid::Uuid;

#[cfg(test)]
mod tests;

fn currency(value: &Value) -> Result<String, Failure> {
    value
        .as_str()
        .filter(|s| apsp_panel_host::money::currency(s))
        .map(str::to_owned)
        .ok_or("invalid_response".into())
}
fn amount(value: &Value) -> Result<String, Failure> {
    let raw = match value {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        _ => return Err("invalid_response".into()),
    };
    let unsigned = raw.strip_prefix('-').unwrap_or(&raw);
    let (whole, fraction) = unsigned.split_once('.').unwrap_or((unsigned, ""));
    let groups: Vec<_> = whole.split(',').collect();
    if raw.len() > 64
        || whole.is_empty()
        || !fraction.bytes().all(|b| b.is_ascii_digit())
        || fraction.len() > 12
        || groups
            .iter()
            .any(|s| s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()))
        || (groups.len() > 1 && (groups[0].len() > 3 || groups[1..].iter().any(|s| s.len() != 3)))
    {
        return Err("invalid_response".into());
    }
    Ok(raw.replace(',', ""))
}
fn costs_service<'a>(cloud: &'a Cloud, account: &Account) -> &'a Aliyun {
    if account.site == "international" {
        &cloud.bss_international
    } else {
        &cloud.bss
    }
}
pub async fn balance(cloud: &Cloud, account: &Account, now: i64) -> Result<Balance, Failure> {
    let resolved = cloud.resolved_account(account).await?;
    let account = &resolved;
    let value = costs_service(cloud, account)
        .call(
            &account.access_key_id,
            &account.access_key_secret,
            "QueryAccountBalance",
            &[],
        )
        .await?;
    Ok(Balance {
        available: amount(&value["Data"]["AvailableAmount"])?,
        currency: currency(&value["Data"]["Currency"])?,
        queried_at: now,
    })
}
pub async fn instance_bill(
    cloud: &Cloud,
    account: &Account,
    r: &Resource,
    now: i64,
) -> Result<InstanceBill, Failure> {
    let resolved = cloud.resolved_account(account).await?;
    let account = &resolved;
    tokio::time::timeout(
        std::time::Duration::from_secs(30),
        instance_bill_pages(cloud, account, r, now),
    )
    .await
    .unwrap_or(Err("request_timeout".into()))
}
async fn instance_bill_pages(
    cloud: &Cloud,
    account: &Account,
    r: &Resource,
    now: i64,
) -> Result<InstanceBill, Failure> {
    let month = billing::month(now);
    let mut rows = Vec::new();
    let mut token = String::new();
    let mut tokens = BTreeSet::new();
    let mut seen = BTreeSet::new();
    let mut total = None;
    for _ in 0..10 {
        let mut params = vec![
            ("BillingCycle", month.clone()),
            ("InstanceID", r.cloud_id.clone()),
            ("Granularity", "MONTHLY".into()),
            ("IsBillingItem", "false".into()),
            ("IsHideZeroCharge", "false".into()),
            ("MaxResults", "100".into()),
        ];
        if !token.is_empty() {
            params.push(("NextToken", token.clone()));
        }
        let value = costs_service(cloud, account)
            .call(
                &account.access_key_id,
                &account.access_key_secret,
                "DescribeInstanceBill",
                &params,
            )
            .await?;
        let data = &value["Data"];
        if data["BillingCycle"] != month {
            return Err("billing_incomplete".into());
        }
        let count = data["TotalCount"]
            .as_u64()
            .ok_or(Failure::from("billing_incomplete"))?;
        if count > 1000 || total.is_some_and(|v| v != count) {
            return Err("billing_incomplete".into());
        }
        total = Some(count);
        let entries = data["Items"]
            .as_array()
            .ok_or(Failure::from("billing_incomplete"))?;
        if entries.len() > 100 {
            return Err("billing_incomplete".into());
        }
        for row in entries {
            if row["InstanceID"] != r.cloud_id || !seen.insert(row.to_string()) {
                return Err("billing_incomplete".into());
            }
            let item = row["Item"]
                .as_str()
                .filter(|s| {
                    matches!(
                        *s,
                        "SubscriptionOrder" | "PayAsYouGoBill" | "Refund" | "Adjustment"
                    )
                })
                .ok_or(Failure::from("invalid_response"))?;
            rows.push(CostRow {
                item: item.into(),
                amount: amount(&row["PretaxAmount"])?,
                currency: currency(&row["Currency"])?,
            });
        }
        token = match &data["NextToken"] {
            Value::Null => String::new(),
            Value::String(s) if s.len() <= 2048 => s.clone(),
            _ => return Err("billing_incomplete".into()),
        };
        if token.is_empty() {
            if rows.len() as u64 != count {
                return Err("billing_incomplete".into());
            }
            return Ok(InstanceBill {
                month,
                queried_at: now,
                rows,
            });
        }
        if entries.is_empty() || rows.len() as u64 >= count || !tokens.insert(token.clone()) {
            return Err("billing_incomplete".into());
        }
    }
    Err("billing_incomplete".into())
}

/// Like the account refresh, the cloud call runs outside the account lock:
/// claim the slot, call, then store unless the account was edited meanwhile.
pub async fn refresh_balance(pool: &PgPool, id: Uuid, cloud: &Cloud, now: i64) -> ApiResult<()> {
    let a = {
        let mut tx = lock(pool, id).await?;
        let a = account_on(&mut tx, id).await?;
        if !a.enabled || !a.balance_enabled || a.balance_next_at > now {
            return Ok(());
        }
        // Claimed until the result is stored; an edit resets it to now.
        sqlx::query("UPDATE alicloud_accounts SET balance_next_at=$2 WHERE id=$1")
            .bind(id)
            .bind(now + 300)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        a
    };
    let result = balance(cloud, &a, now).await;
    let mut tx = lock(pool, id).await?;
    if !unchanged_account(&mut tx, id, a.revision).await? {
        return Ok(());
    }
    match result {
        Ok(value) => {
            sqlx::query("UPDATE alicloud_accounts SET balance=$2,balance_error=NULL,balance_next_at=$3 WHERE id=$1").bind(id).bind(Json(value)).bind(now+21600).execute(&mut *tx).await?;
        }
        Err(error) => {
            sqlx::query(
                "UPDATE alicloud_accounts SET balance_error=$2,balance_next_at=$3 WHERE id=$1",
            )
            .bind(id)
            .bind(error.code)
            .bind(now + error.retry_after.max(300))
            .execute(&mut *tx)
            .await?;
        }
    }
    tx.commit().await?;
    Ok(())
}
pub async fn refresh_bill(pool: &PgPool, id: Uuid, cloud: &Cloud, now: i64) -> ApiResult<()> {
    let initial = apsp_resources::cloud::resource(pool, id).await?;
    let (a, r) = {
        let mut tx = lock(pool, initial.account_id).await?;
        let a = account_on(&mut tx, initial.account_id).await?;
        let r: Resource =
            sqlx::query_as("SELECT * FROM alicloud_resources WHERE id=$1 AND NOT archived")
                .bind(id)
                .fetch_one(&mut *tx)
                .await?;
        let stale_month = r
            .instance_bill
            .as_ref()
            .is_some_and(|b| b.month != billing::month(now));
        if !a.enabled
            || !a.billing_enabled
            || (r.bill_next_at > now && !(stale_month && r.bill_error.is_none()))
        {
            return Ok(());
        }
        sqlx::query(
            "UPDATE alicloud_resources SET bill_next_at=GREATEST(bill_next_at,$2) WHERE id=$1",
        )
        .bind(id)
        .bind(now + 300)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        (a, r)
    };
    let result = instance_bill(cloud, &a, &r, now).await;
    let mut tx = lock(pool, initial.account_id).await?;
    // Resource edits change only its name and bandwidth policy, not its bill.
    let present: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM alicloud_resources WHERE id=$1 AND NOT archived)",
    )
    .bind(id)
    .fetch_one(&mut *tx)
    .await?;
    if !present || !unchanged_account(&mut tx, initial.account_id, a.revision).await? {
        return Ok(());
    }
    match result {
        Ok(value) => {
            sqlx::query("UPDATE alicloud_resources SET instance_bill=$2,bill_error=NULL,bill_next_at=$3 WHERE id=$1").bind(id).bind(Json(value)).bind(now+21600).execute(&mut *tx).await?;
        }
        Err(error) => {
            sqlx::query("UPDATE alicloud_resources SET bill_error=$2,bill_next_at=$3 WHERE id=$1")
                .bind(id)
                .bind(error.code)
                .bind(now + error.retry_after.max(300))
                .execute(&mut *tx)
                .await?;
        }
    }
    tx.commit().await?;
    Ok(())
}
/// Whether the account still exists, enabled, at the revision a result was read for.
async fn unchanged_account(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    id: Uuid,
    revision: i64,
) -> ApiResult<bool> {
    Ok(sqlx::query_scalar::<_, i64>(
        "SELECT revision FROM alicloud_accounts WHERE id=$1 AND NOT archived AND enabled",
    )
    .bind(id)
    .fetch_optional(&mut **tx)
    .await?
        == Some(revision))
}
pub async fn tick(pool: &PgPool, cloud: &Cloud) -> ApiResult<()> {
    let now = apsp_protocol::now_timestamp();
    let accounts:Vec<Uuid>=sqlx::query_scalar("SELECT id FROM alicloud_accounts WHERE enabled AND balance_enabled AND NOT archived AND balance_next_at<=$1 ORDER BY balance_next_at,id LIMIT 1").bind(now).fetch_all(pool).await?;
    for id in accounts {
        refresh_balance(pool, id, cloud, now).await?;
    }
    let resources:Vec<Uuid>=sqlx::query_scalar("SELECT r.id FROM alicloud_resources r JOIN alicloud_accounts a ON a.id=r.account_id WHERE a.enabled AND a.billing_enabled AND NOT a.archived AND NOT r.archived AND (r.bill_next_at<=$1 OR (r.instance_bill->>'month'<>$2 AND r.bill_error IS NULL)) ORDER BY r.bill_next_at,r.id LIMIT 1").bind(now).bind(billing::month(now)).fetch_all(pool).await?;
    for id in resources {
        refresh_bill(pool, id, cloud, now).await?;
    }
    Ok(())
}

/// Stores the CDT bill an account refresh read, or the reason it could not: a failure
/// becomes the account's error and defers the next pass.
pub async fn store_bill(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    account: &mut Account,
    bill: Result<apsp_resources::cloud::model::Bill, Failure>,
    now: i64,
) -> ApiResult<()> {
    match bill {
        Ok(bill) => {
            sqlx::query("UPDATE alicloud_accounts SET bill=$2,error_code=NULL WHERE id=$1")
                .bind(account.id)
                .bind(Json(&bill))
                .execute(&mut **tx)
                .await?;
            account.bill = Some(Json(bill));
            account.error_code = None;
        }
        Err(error) => {
            sqlx::query("UPDATE alicloud_accounts SET error_code=$2,next_run_at=GREATEST(next_run_at,$3) WHERE id=$1")
                .bind(account.id).bind(error.code).bind(now+error.retry_after.max(300)).execute(&mut **tx).await?;
            account.error_code = Some(error.code.into());
        }
    }
    Ok(())
}
