use super::super::super::{
    client::Cloud,
    model::{self, Account},
};
use super::{MAX_BYTES, key_digest};
use crate::cloud_api::Failure;
use serde_json::Value;
use std::collections::BTreeSet;

pub(super) fn pool(region: &str) -> Result<&'static str, Failure> {
    if !model::identifier(region, "") || !region.starts_with(|c: char| c.is_ascii_lowercase()) {
        return Err("invalid_response".into());
    }
    Ok(if region.starts_with("cn-") && region != "cn-hongkong" {
        "mainland"
    } else {
        "non-mainland"
    })
}

fn bytes(value: &Value) -> Result<u64, Failure> {
    value
        .as_u64()
        .or_else(|| {
            value
                .as_str()
                .filter(|v| !v.is_empty() && v.bytes().all(|c| c.is_ascii_digit()))
                .and_then(|v| v.parse().ok())
        })
        .filter(|v| *v <= MAX_BYTES)
        .ok_or("invalid_response".into())
}

fn complete(value: &Value, count: usize) -> Result<(), Failure> {
    if value
        .get("NextToken")
        .is_some_and(|v| !v.is_null() && v != "")
        || value
            .get("TotalCount")
            .is_some_and(|v| v.as_u64() != Some(count as u64))
    {
        return Err("invalid_response".into());
    }
    Ok(())
}

pub(super) fn traffic(value: &Value, region: &str) -> Result<u64, Failure> {
    let rows = value["TrafficDetails"]
        .as_array()
        .filter(|v| v.len() <= 256)
        .ok_or(Failure::from("invalid_response"))?;
    complete(value, rows.len())?;
    let target_pool = pool(region)?;
    let mut seen = BTreeSet::new();
    let mut total = 0_u64;
    for row in rows {
        let region = row["BusinessRegionId"]
            .as_str()
            .ok_or(Failure::from("invalid_response"))?;
        let row_pool = pool(region)?;
        let isp = row["ISPType"]
            .as_str()
            .filter(|s| !s.is_empty() && s.len() <= 64)
            .ok_or(Failure::from("invalid_response"))?
            .to_ascii_lowercase();
        if !seen.insert((region, isp.clone())) {
            return Err("invalid_response".into());
        }
        let amount = bytes(&row["Traffic"])?;
        if row_pool != target_pool {
            continue;
        }
        if isp != "bgp" {
            if amount != 0 {
                return Err("cdt_unsupported_isp".into());
            }
            continue;
        }
        let products = row["ProductTrafficDetails"]
            .as_array()
            .filter(|v| v.len() <= 256)
            .ok_or(Failure::from("invalid_response"))?;
        let mut subtotal = 0_u64;
        let mut product_names = BTreeSet::new();
        for product in products {
            let name = product["Product"]
                .as_str()
                .filter(|v| !v.is_empty() && v.len() <= 128)
                .ok_or(Failure::from("invalid_response"))?;
            if !product_names.insert(name) {
                return Err("invalid_response".into());
            }
            subtotal = subtotal
                .checked_add(bytes(&product["Traffic"])?)
                .ok_or(Failure::from("invalid_response"))?;
        }
        if subtotal != amount {
            return Err("invalid_response".into());
        }
        total = total
            .checked_add(amount)
            .filter(|v| *v <= MAX_BYTES)
            .ok_or(Failure::from("invalid_response"))?;
    }
    Ok(total)
}

impl Cloud {
    pub(in super::super::super) async fn guard_usage(
        &self,
        account: &Account,
        region: &str,
    ) -> Result<(u64, String), Failure> {
        let resolved = self.resolved_account(account).await?;
        let value = self
            .cdt
            .call(
                &resolved.access_key_id,
                &resolved.access_key_secret,
                "ListCdtInternetTraffic",
                &[("RegionId", region.into())],
            )
            .await?;
        // BusinessRegionId is deliberately absent: BGP allowance is shared by pool.
        let usage = traffic(&value, region)?;
        if usage == 0 {
            let switched = self
                .cdt
                .call(
                    &resolved.access_key_id,
                    &resolved.access_key_secret,
                    "ListSwitchedCdtProducts",
                    &[
                        ("RegionId", region.into()),
                        ("BusinessRegionId", region.into()),
                    ],
                )
                .await?;
            let products = switched["CdtProducts"]
                .as_array()
                .filter(|v| v.len() <= 256)
                .ok_or(Failure::from("invalid_response"))?;
            complete(&switched, products.len())?;
            if !products.iter().any(|p| {
                p["BusinessRegionId"] == region
                    && p["CdtType"] == "INTERNET"
                    && matches!(
                        p["BillingType"].as_str(),
                        Some("PayByTraffic" | "PayByDominantTraffic")
                    )
            }) {
                return Err("cdt_not_enabled".into());
            }
        }
        Ok((usage, key_digest(&resolved.access_key_id)))
    }
}
