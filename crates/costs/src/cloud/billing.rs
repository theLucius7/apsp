use crate::cloud::accounts::{Bill, BillRow, Traffic, TrafficRegion};
use apsp_cloud_api::{Failure, aliyun::Aliyun};
use apsp_panel_host::text::Text;
use apsp_resources::cloud::{
    client::{Cloud, Keys},
    model::Account,
};
use serde_json::Value;
use std::collections::BTreeSet;

pub fn month(now: i64) -> String {
    apsp_panel_host::time::iso8601(now.saturating_add(8 * 3600))[..7].into()
}

fn field(value: &Value, key: &str) -> Result<String, Failure> {
    match &value[key] {
        Value::String(v) if apsp_panel_host::text::Text::bytes(256).accepts(v) => Ok(v.clone()),
        Value::Number(v) => Ok(v.to_string()),
        Value::Null => Ok(String::new()),
        _ => Err("invalid_response".into()),
    }
}

// Decimal parsing avoids floating point rounding crossing a control threshold.
fn micro_gb(value: &str) -> Option<u64> {
    let (whole, fraction) = value.split_once('.').unwrap_or((value, ""));
    if whole.is_empty()
        || !whole.bytes().all(|v| v.is_ascii_digit())
        || !fraction.bytes().all(|v| v.is_ascii_digit())
        || fraction.len() > 12
    {
        return None;
    }
    let whole: u64 = whole.parse().ok()?;
    let digits = &fraction[..fraction.len().min(6)];
    let tail = if digits.is_empty() {
        0
    } else {
        digits.parse::<u64>().ok()?
    };
    whole
        .checked_mul(1_000_000)?
        .checked_add(tail.checked_mul(10_u64.pow(6 - digits.len() as u32))?)
}

pub async fn query(
    client: &Aliyun,
    account: &Account,
    keys: &Keys,
    now: i64,
) -> Result<Bill, Failure> {
    let cycle = month(now);
    let mut rows = Vec::new();
    let mut total_count = None;
    let mut total = Some(0_u64);
    let mut identities = BTreeSet::new();
    for page in 1..=10 {
        let result = client
            .call(
                &keys.access_key_id,
                &keys.access_key_secret,
                "QueryInstanceBill",
                &[
                    (
                        "RegionId",
                        if account.site == "international" {
                            "ap-southeast-1"
                        } else {
                            "cn-hangzhou"
                        }
                        .into(),
                    ),
                    ("BillingCycle", cycle.clone()),
                    ("ProductCode", "cdt".into()),
                    ("IsBillingItem", "true".into()),
                    ("Granularity", "MONTHLY".into()),
                    ("IsHideZeroCharge", "false".into()),
                    ("PageNum", page.to_string()),
                    ("PageSize", "100".into()),
                ],
            )
            .await?;
        let data = &result["Data"];
        if data["BillingCycle"] != cycle || data["PageNum"] != page {
            return Err("billing_incomplete".into());
        }
        let count = data["TotalCount"]
            .as_u64()
            .ok_or(Failure::from("billing_incomplete"))?;
        if count > 1000 || total_count.is_some_and(|v| v != count) {
            return Err("billing_incomplete".into());
        }
        total_count = Some(count);
        let entries = data["Items"]["Item"]
            .as_array()
            .ok_or(Failure::from("billing_incomplete"))?;
        if entries.len() > 100 || (entries.is_empty() && count > rows.len() as u64) {
            return Err("billing_incomplete".into());
        }
        for entry in entries {
            if entry["ProductCode"] != "cdt" {
                return Err("billing_incomplete".into());
            }
            let row = BillRow {
                instance_id: field(entry, "InstanceID")?,
                region: field(entry, "Region")?,
                product_type: field(entry, "ProductType")?,
                billing_item: field(entry, "BillingItem")?,
                usage: field(entry, "Usage")?,
                unit: field(entry, "UsageUnit")?,
                amount: field(entry, "PretaxAmount")?,
                currency: field(entry, "Currency")?,
            };
            // Repeated rows indicate unstable pagination rather than extra consumption.
            // Monthly rows are aggregates of these billing dimensions. A changed
            // nickname, amount or usage on a repeated page is not another charge.
            let key = serde_json::to_string(&(
                &row.instance_id,
                &row.region,
                &row.product_type,
                &row.billing_item,
                field(entry, "Item")?,
                &row.currency,
                field(entry, "OwnerID")?,
                field(entry, "SubscriptionType")?,
            ))
            .map_err(|_| Failure::from("invalid_response"))?;
            if !identities.insert(key) {
                return Err("billing_incomplete".into());
            }
            total = total.and_then(|sum| {
                (row.unit == "GB"
                    && entry["Item"] == "PayAsYouGoBill"
                    && !row.instance_id.is_empty())
                .then_some(())
                .and_then(|_| micro_gb(&row.usage))
                .and_then(|v| sum.checked_add(v))
            });
            rows.push(row);
        }
        if rows.len() as u64 == count {
            return Ok(Bill {
                month: cycle,
                queried_at: now,
                usage_micro_gb: if rows.is_empty() { None } else { total },
                rows,
            });
        }
        if rows.len() as u64 > count {
            return Err("billing_incomplete".into());
        }
    }
    Err("billing_incomplete".into())
}

fn valid_code(value: &Value) -> bool {
    value == "Success" || value == "200" || value == 200
}

fn traffic_bytes(value: &Value) -> Option<u64> {
    if let Some(v) = value.as_u64() {
        return Some(v);
    }
    let text = value.as_str()?;
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

pub fn parse_traffic(result: &Value, now: i64) -> Result<Traffic, Failure> {
    if !result.is_object() {
        return Err("invalid_response".into());
    }
    let Some(request_id) = result["RequestId"].as_str() else {
        return Err("invalid_response".into());
    };
    if !Text::bytes(128).not_blank().accepts(request_id) {
        return Err("invalid_response".into());
    }
    if result.get("Data").is_some_and(|v| !v.is_object()) {
        return Err("invalid_response".into());
    }
    let top = result.get("TrafficDetails");
    let nested = result["Data"].get("TrafficDetails");
    let details = match (top, nested) {
        (Some(v), None) | (None, Some(v)) => v,
        (Some(a), Some(b)) if a == b => a,
        _ => return Err("invalid_response".into()),
    };
    let entries = details
        .as_array()
        .filter(|v| v.len() <= 256)
        .ok_or(Failure::from("invalid_response"))?;
    // A nested compatibility response must prove the same complete snapshot.
    // Metadata at either level can advertise additional pages or provider errors; never discard it.
    for metadata in [result, &result["Data"]] {
        let bad_code = metadata.get("Code").is_some_and(|v| !valid_code(v));
        let bad_message = metadata.get("Message").is_some();
        let bad_success = metadata.get("Success").is_some_and(|v| v != true);
        let bad_token = metadata
            .get("NextToken")
            .is_some_and(|v| !v.is_null() && v.as_str() != Some(""));
        let bad_count = metadata
            .get("TotalCount")
            .is_some_and(|v| v.as_u64() != Some(entries.len() as u64));
        if bad_code || bad_message || bad_success || bad_token || bad_count {
            return Err("invalid_response".into());
        }
    }
    let mut seen = BTreeSet::new();
    let (mut mainland, mut overseas) = (0_u64, 0_u64);
    let mut regions = Vec::new();
    for item in entries {
        if !item.is_object() {
            return Err("invalid_response".into());
        }
        let region = field(item, "BusinessRegionId")?;
        if !apsp_resources::cloud::model::identifier(&region, "") || !seen.insert(region.clone()) {
            return Err("invalid_response".into());
        }
        let bytes = traffic_bytes(&item["Traffic"]).ok_or(Failure::from("invalid_response"))?;
        let sum = if region.starts_with("cn-") && region != "cn-hongkong" {
            &mut mainland
        } else {
            &mut overseas
        };
        *sum = sum
            .checked_add(bytes)
            .ok_or(Failure::from("invalid_response"))?;
        regions.push(TrafficRegion {
            region,
            bytes: bytes.to_string(),
        });
    }
    regions.sort_by(|a, b| a.region.cmp(&b.region));
    Ok(Traffic {
        queried_at: now,
        mainland_bytes: mainland.to_string(),
        overseas_bytes: overseas.to_string(),
        regions,
    })
}

/// The CDT bill of an account, read through the account's cloud client.
pub async fn bill(cloud: &Cloud, account: &Account, now: i64) -> Result<Bill, Failure> {
    let keys = cloud.keys(account).await?;
    let service = if account.site == "international" {
        &cloud.bss_international
    } else {
        &cloud.bss
    };
    tokio::time::timeout(
        std::time::Duration::from_secs(30),
        query(service, account, &keys, now),
    )
    .await
    .unwrap_or(Err("request_timeout".into()))
}
/// The CDT traffic of the account the keys belong to.
pub async fn traffic(cloud: &Cloud, keys: &Keys, now: i64) -> Result<Traffic, Failure> {
    let result = cloud
        .cdt
        .call(
            &keys.access_key_id,
            &keys.access_key_secret,
            "ListCdtInternetTraffic",
            &[("RegionId", "cn-hongkong".into())],
        )
        .await?;
    parse_traffic(&result, now)
}

#[cfg(test)]
mod tests {
    use super::parse_traffic;
    use serde_json::json;

    #[test]
    fn valid_empty_cdt_traffic_records_zero_usage_and_fresh_timestamp() {
        for valid in [
            json!({"RequestId": "C4A45FE1-RNA-EMPTY", "TrafficDetails": []}),
            json!({
                "RequestId": "C4A45FE1-RNA-COMPLETE",
                "TrafficDetails": [],
                "TotalCount": 0,
                "NextToken": ""
            }),
            json!({
                "RequestId": "C4A45FE1-RNA-LAST-PAGE",
                "TrafficDetails": [],
                "NextToken": null
            }),
            json!({
                "RequestId": "C4A45FE1-RNA-NESTED",
                "Code": "Success",
                "Success": true,
                "Data": {"TrafficDetails": [], "TotalCount": 0, "NextToken": ""}
            }),
        ] {
            let parsed = parse_traffic(&valid, 1_700_000_123).unwrap();
            assert_eq!(parsed.queried_at, 1_700_000_123);
            assert_eq!(parsed.mainland_bytes, "0");
            assert_eq!(parsed.overseas_bytes, "0");
            assert!(parsed.regions.is_empty());
        }
    }

    #[test]
    fn malformed_incomplete_or_error_cdt_responses_never_invent_zero() {
        let row = json!({"BusinessRegionId": "cn-hongkong", "Traffic": "1024"});
        for invalid in [
            json!(null),
            json!([]),
            json!({"TrafficDetails": []}),
            json!({"RequestId": "", "TrafficDetails": []}),
            json!({"RequestId": "   ", "TrafficDetails": []}),
            json!({"RequestId": 123, "TrafficDetails": []}),
            json!({"RequestId": "bad\nid", "TrafficDetails": []}),
            json!({"RequestId": "r1"}),
            json!({"RequestId": "r1", "Data": {}}),
            json!({"RequestId": "r1", "Data": null, "TrafficDetails": []}),
            json!({"RequestId": "r1", "Data": "invalid", "TrafficDetails": []}),
            json!({"RequestId": "r1", "TrafficDetails": null}),
            json!({"RequestId": "r1", "TrafficDetails": {}}),
            json!({"RequestId": "r1", "TrafficDetails": "[]"}),
            json!({"RequestId": "r1", "TrafficDetails": [], "Data": {"TrafficDetails": [row]}}),
            json!({"RequestId": "r1", "Code": null, "TrafficDetails": []}),
            json!({"RequestId": "r1", "Code": "InternalError", "TrafficDetails": []}),
            json!({"RequestId": "r1", "Code": 500, "TrafficDetails": []}),
            json!({"RequestId": "r1", "Message": null, "TrafficDetails": []}),
            json!({"RequestId": "r1", "Message": "service error", "TrafficDetails": []}),
            json!({"RequestId": "r1", "Success": null, "TrafficDetails": []}),
            json!({"RequestId": "r1", "Success": false, "TrafficDetails": []}),
            json!({"RequestId": "r1", "Success": "true", "TrafficDetails": []}),
            json!({"RequestId": "r1", "Data": {"Code": "Throttling", "TrafficDetails": []}}),
            json!({"RequestId": "r1", "Data": {"Message": "denied", "TrafficDetails": []}}),
            json!({"RequestId": "r1", "Data": {"Success": false, "TrafficDetails": []}}),
            json!({"RequestId": "r1", "TrafficDetails": [], "NextToken": "more"}),
            json!({"RequestId": "r1", "TrafficDetails": [], "NextToken": 1}),
            json!({"RequestId": "r1", "Data": {"TrafficDetails": [], "NextToken": "more"}}),
            json!({"RequestId": "r1", "Data": {"TrafficDetails": [], "NextToken": 1}}),
            json!({"RequestId": "r1", "TrafficDetails": [], "TotalCount": null}),
            json!({"RequestId": "r1", "TrafficDetails": [], "TotalCount": 2}),
            json!({"RequestId": "r1", "TrafficDetails": [], "TotalCount": "0"}),
            json!({"RequestId": "r1", "TrafficDetails": [], "TotalCount": -1}),
            json!({"RequestId": "r1", "Data": {"TrafficDetails": [], "TotalCount": null}}),
            json!({"RequestId": "r1", "Data": {"TrafficDetails": [], "TotalCount": 1}}),
            json!({"RequestId": "r1", "Data": {"TrafficDetails": [], "TotalCount": "0"}}),
        ] {
            let err = parse_traffic(&invalid, 1_700_000_123).unwrap_err();
            assert_eq!(err.code, "invalid_response", "{invalid}");
        }
    }
}
