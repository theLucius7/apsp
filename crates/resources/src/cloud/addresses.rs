//! Optional address discovery; never used to authorize cloud mutations.
use super::{
    client::Cloud,
    model::{Account, Resource},
};
use apsp_cloud_api::Failure;
use serde_json::Value;
use std::{collections::BTreeSet, net::Ipv6Addr, time::Duration};

fn collect(value: &Value, output: &mut BTreeSet<String>, depth: usize) {
    if depth > 10 || output.len() >= 64 {
        return;
    }
    match value {
        Value::String(raw) => {
            if let Ok(ip) = raw.parse::<Ipv6Addr>()
                && !ip.is_unspecified()
                && !ip.is_loopback()
                && !ip.is_multicast()
                && !ip.is_unicast_link_local()
                && !ip.is_unique_local()
            {
                output.insert(ip.to_string());
            }
        }
        Value::Array(rows) => {
            for row in rows.iter().take(128) {
                collect(row, output, depth + 1);
            }
        }
        Value::Object(row) => {
            for key in [
                "Ipv6Address",
                "Ipv6Addresses",
                "Ipv6AddressSets",
                "Ipv6AddressSet",
                "Ipv6Sets",
                "Ipv6Set",
                "NetworkInterfaces",
                "NetworkInterface",
                "NetworkInterfaceSets",
                "NetworkInterfaceSet",
            ] {
                if let Some(value) = row.get(key) {
                    collect(value, output, depth + 1);
                }
            }
        }
        _ => {}
    }
}
pub fn from_instance(row: &Value) -> Vec<String> {
    let mut addresses = BTreeSet::new();
    collect(row, &mut addresses, 0);
    addresses.into_iter().collect()
}
impl Cloud {
    pub async fn ipv6(
        &self,
        account: &Account,
        resource: &Resource,
        inline: &[String],
    ) -> Result<Vec<String>, Failure> {
        if !inline.is_empty() {
            return Ok(inline.to_vec());
        }
        tokio::time::timeout(Duration::from_secs(3), self.network_ipv6(account, resource))
            .await
            .unwrap_or_else(|_| Err("request_timeout".into()))
    }
    async fn network_ipv6(
        &self,
        account: &Account,
        resource: &Resource,
    ) -> Result<Vec<String>, Failure> {
        let keys = self.keys(account).await?;
        let mut addresses = BTreeSet::new();
        let mut tokens = BTreeSet::new();
        let mut token = String::new();
        for _ in 0..8 {
            let mut params = vec![
                ("RegionId", resource.region.clone()),
                ("InstanceId", resource.cloud_id.clone()),
                ("MaxResults", "100".into()),
            ];
            if !token.is_empty() {
                params.push(("NextToken", token.clone()));
            }
            let value = self
                .ecs
                .call(
                    &keys.access_key_id,
                    &keys.access_key_secret,
                    "DescribeNetworkInterfaces",
                    &params,
                )
                .await?;
            let sets = &value["NetworkInterfaceSets"];
            let rows = sets
                .as_array()
                .or_else(|| sets["NetworkInterfaceSet"].as_array())
                .ok_or(Failure::from("invalid_response"))?;
            if rows.len() > 100 {
                return Err("invalid_response".into());
            }
            for row in rows {
                if row
                    .get("InstanceId")
                    .and_then(Value::as_str)
                    .is_some_and(|id| id != resource.cloud_id)
                {
                    return Err("invalid_response".into());
                }
                collect(row, &mut addresses, 0);
            }
            token = match value.get("NextToken") {
                None | Some(Value::Null) => String::new(),
                Some(Value::String(token)) => token.clone(),
                _ => return Err("invalid_response".into()),
            };
            if token.is_empty() {
                return Ok(addresses.into_iter().collect());
            }
            if token.len() > 2048 || !tokens.insert(token.clone()) {
                return Err("invalid_response".into());
            }
        }
        Err("invalid_response".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn nested_addresses_are_normalized_and_do_not_include_ipv4_or_private_addresses() {
        let row = json!({"Ipv6Address":["2001:db8::1", "::1"],"NetworkInterfaces":{"NetworkInterface":[{"Ipv6Sets":{"ignored":"2001:db8::9"},"Ipv6AddressSets":{"Ipv6AddressSet":[{"Ipv6Address":"2001:0db8::1"},{"Ipv6Address":"2001:db8::2"},{"Ipv6Address":"fe80::1"},{"Ipv6Address":"192.0.2.1"}]}}]}});
        assert_eq!(from_instance(&row), vec!["2001:db8::1", "2001:db8::2"]);
    }

    #[sqlx::test(migrations = "../panel/migrations")]
    async fn fallback_scopes_pages_to_instance_and_rejects_repeated_tokens(pool: sqlx::PgPool) {
        use apsp_cloud_api::test_support::{Mock, Reply};
        let (id, rid) = (uuid::Uuid::new_v4(), uuid::Uuid::new_v4());
        sqlx::query("WITH credential AS (INSERT INTO credential_entries(id,name,kind,key_id,nonce,ciphertext,created_at,updated_at) VALUES(gen_random_uuid(),'TEST_ONLY','cloud','TEST_ONLY',decode(repeat('00',12),'hex'),decode('00','hex'),0,0) RETURNING id) INSERT INTO alicloud_accounts(id,name,credential_id) SELECT $1,'测试账号',id FROM credential")
            .bind(id).execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO alicloud_resources(id,account_id,name,kind,region,cloud_id) VALUES($1,$2,'测试资源','ecs','cn-hangzhou','i-testonly')")
            .bind(rid).bind(id).execute(&pool).await.unwrap();
        let account: Account = sqlx::query_as("SELECT * FROM alicloud_accounts WHERE id=$1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
        let resource = super::super::resource(&pool, rid).await.unwrap();
        let page = |token: &str, address: &str| {
            Reply::ok(
                "DescribeNetworkInterfaces",
                json!({"RequestId":"test","NextToken":token,"NetworkInterfaceSets":[{"InstanceId":"i-testonly","Ipv6Sets":[{"Ipv6Address":address}]}]}),
            )
        };
        let mock = Mock::start(vec![page("next", "2001:db8::1"), page("", "2001:db8::2")]).await;
        let result = Cloud::local(&mock.endpoint)
            .ipv6(&account, &resource, &[])
            .await
            .unwrap();
        assert_eq!(result, vec!["2001:db8::1", "2001:db8::2"]);
        for request in mock.requests() {
            assert_eq!(request.params["InstanceId"], "i-testonly");
        }
        mock.exhausted();
        let mock = Mock::start(vec![
            page("next", "2001:db8::1"),
            page("next", "2001:db8::2"),
        ])
        .await;
        assert!(
            Cloud::local(&mock.endpoint)
                .ipv6(&account, &resource, &[])
                .await
                .is_err()
        );
        mock.exhausted();
        let mock = Mock::start(vec![]).await;
        assert_eq!(
            Cloud::local(&mock.endpoint)
                .ipv6(&account, &resource, &result)
                .await
                .unwrap(),
            result
        );
        assert!(mock.requests().is_empty());
    }
}
