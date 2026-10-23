use anyhow::{Context, Result, bail};
use apsp_adapter_sdk::{Counter, RuntimeSpec};
use std::{
    collections::{BTreeMap, BTreeSet},
    time::Duration,
};
use tonic::{Request, client::Grpc, codec::ProstCodec, transport::Endpoint};

#[allow(dead_code)]
mod proto {
    tonic::include_proto!("experimental.v2rayapi");
}

const QUERY_PATH: &str = "/v2ray.core.app.stats.command.StatsService/QueryStats";

pub(crate) async fn query(address: &str) -> Result<Vec<proto::Stat>> {
    let address = crate::native::stats_address(address)?;
    tokio::time::timeout(Duration::from_secs(3), async {
        let channel = Endpoint::from_shared(format!("http://{address}"))?
            .connect_timeout(Duration::from_secs(1))
            .timeout(Duration::from_secs(2))
            .connect()
            .await
            .context("connect to statistics API")?;
        let mut client = Grpc::new(channel).max_decoding_message_size(16 * 1024 * 1024);
        client
            .ready()
            .await
            .context("statistics API is not ready")?;
        let request = Request::new(proto::QueryStatsRequest {
            pattern: String::new(),
            patterns: vec!["user>>>".into()],
            reset: false,
            regexp: false,
        });
        let response: tonic::Response<proto::QueryStatsResponse> = client
            .unary(
                request,
                tonic::codegen::http::uri::PathAndQuery::from_static(QUERY_PATH),
                ProstCodec::default(),
            )
            .await
            .context("query statistics API")?;
        Ok(response.into_inner().stat)
    })
    .await
    .context("statistics request timed out")?
}

pub(crate) fn configured_users(runtime: &RuntimeSpec) -> Result<BTreeSet<String>> {
    let config: serde_json::Value = serde_json::from_str(
        runtime
            .files
            .get("config.json")
            .context("missing config.json")?,
    )?;
    let users = config["experimental"]["v2ray_api"]["stats"]["users"]
        .as_array()
        .context("configured statistics users must be an array")?;
    let mut configured = BTreeSet::new();
    for user in users {
        let name = user
            .as_str()
            .context("invalid configured statistics user")?;
        if name.is_empty()
            || name.len() > 512
            || name.contains(">>>")
            || name.chars().any(char::is_control)
            || !configured.insert(name.to_owned())
        {
            bail!("invalid or duplicate configured statistics user");
        }
    }
    Ok(configured)
}

pub(crate) fn counters(
    stats: Vec<proto::Stat>,
    configured: BTreeSet<String>,
) -> Result<Vec<Counter>> {
    // The upstream creates counters lazily. A successful query omitting a configured
    // user means zero in this generation; an RPC failure must remain an error.
    let mut counters: BTreeMap<_, _> = configured
        .into_iter()
        .map(|name| {
            let counter = Counter {
                stat_name: name.clone(),
                ..Counter::default()
            };
            (name, counter)
        })
        .collect();
    let mut seen = BTreeSet::new();
    for stat in stats {
        let Some(user) = stat.name.strip_prefix("user>>>") else {
            continue;
        };
        let (name, direction) = user
            .split_once(">>>traffic>>>")
            .context("malformed user counter")?;
        if name.is_empty() || name.contains(">>>") || !matches!(direction, "uplink" | "downlink") {
            bail!("malformed user counter");
        }
        let Some(counter) = counters.get_mut(name) else {
            continue;
        };
        if !seen.insert(stat.name.clone()) {
            bail!("duplicate user counter");
        }
        let value: u64 = stat.value.try_into().context("negative user counter")?;
        match direction {
            "uplink" => counter.uplink = value,
            "downlink" => counter.downlink = value,
            _ => unreachable!(),
        }
    }
    Ok(counters.into_values().collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stat(name: &str, value: i64) -> proto::Stat {
        proto::Stat {
            name: name.into(),
            value,
        }
    }

    #[test]
    fn parses_user_totals_without_counting_inbounds_twice() {
        let totals = counters(
            vec![
                stat("user>>>u2_n3>>>traffic>>>downlink", 15),
                stat("inbound>>>node-3>>>traffic>>>uplink", 100),
                stat("user>>>u1_n3>>>traffic>>>uplink", 5),
                stat("user>>>u1_n3>>>traffic>>>downlink", 10),
            ],
            ["u1_n3".into(), "u2_n3".into()].into(),
        )
        .unwrap();
        assert_eq!(
            totals,
            vec![
                Counter {
                    stat_name: "u1_n3".into(),
                    uplink: 5,
                    downlink: 10
                },
                Counter {
                    stat_name: "u2_n3".into(),
                    uplink: 0,
                    downlink: 15
                },
            ]
        );
    }

    #[test]
    fn missing_configured_users_are_zero_and_unconfigured_users_are_ignored() {
        let configured = ["u1_n3".into(), "u2_n3".into()].into();
        let totals = counters(
            vec![
                stat("user>>>u1_n3>>>traffic>>>uplink", 5),
                stat("user>>>unexpected>>>traffic>>>downlink", 100),
            ],
            configured,
        )
        .unwrap();
        assert_eq!(
            totals,
            vec![
                Counter {
                    stat_name: "u1_n3".into(),
                    uplink: 5,
                    downlink: 0
                },
                Counter {
                    stat_name: "u2_n3".into(),
                    uplink: 0,
                    downlink: 0
                },
            ]
        );
        assert_eq!(
            counters(vec![], ["u1_n3".into()].into()).unwrap(),
            vec![Counter {
                stat_name: "u1_n3".into(),
                uplink: 0,
                downlink: 0
            },]
        );
    }

    #[test]
    fn configured_users_come_only_from_the_statistics_allowlist() {
        let mut runtime = RuntimeSpec {
            revision: 1,
            kernel_version: "1.14.2".into(),
            config_hash: String::new(),
            binary_path: "/tmp/fixture-runtime".into(),
            revision_dir: "/tmp/fixture-revision".into(),
            stats_listen: "127.0.0.1:18085".into(),
            files: BTreeMap::new(),
        };
        for users in [
            serde_json::json!(["u1_n3"]),
            serde_json::json!(["u1_n3", "u1_n3"]),
            serde_json::json!([null]),
        ] {
            runtime.files.insert(
                "config.json".into(),
                serde_json::json!({
                    "inbounds": [{"users": [{"name": "outside-list"}]}],
                    "experimental": {"v2ray_api": {"stats": {"users": users}}}
                })
                .to_string(),
            );
            if users == serde_json::json!(["u1_n3"]) {
                assert_eq!(configured_users(&runtime).unwrap(), ["u1_n3".into()].into());
            } else {
                assert!(configured_users(&runtime).is_err());
            }
        }
    }

    #[test]
    fn rejects_negative_duplicate_and_malformed_counters() {
        let name = "user>>>u1_n3>>>traffic>>>uplink";
        assert!(counters(vec![stat(name, -1)], ["u1_n3".into()].into()).is_err());
        assert!(counters(vec![stat(name, 1), stat(name, 2)], ["u1_n3".into()].into()).is_err());
        for name in [
            "user>>>",
            "user>>>>>>traffic>>>uplink",
            "user>>>u1_n3>>>traffic>>>other",
            "user>>>bad>>>name>>>traffic>>>uplink",
        ] {
            assert!(counters(vec![stat(name, 1)], ["u1_n3".into()].into()).is_err());
        }
    }
}
