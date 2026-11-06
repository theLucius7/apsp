use super::super::{
    client::Cloud,
    model::{Account, Resource, identifier},
};
use crate::cloud_api::{
    Failure,
    globalping::{Globalping, ProbeResult},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::time::Duration;
use uuid::Uuid;

/// Pay-by-traffic EIPs accept at most this peak while allocated; a shared
/// bandwidth package replaces it once the address joins.
const PACKAGE_ALLOCATION_MBPS: i64 = 200;
const PROBE_COUNTRY: &str = "CN";

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Instance {
    pub status: String,
    pub attached_eip: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Eip {
    pub allocation_id: String,
    pub ip: String,
    pub status: String,
    pub instance_id: Option<String>,
    pub package_id: Option<String>,
    pub isp: String,
    pub netmode: String,
    pub internet_charge_type: String,
    pub charge_type: String,
    pub bandwidth: i64,
    pub mode: String,
    pub business_status: String,
    pub deletion_protection: bool,
    pub security_protection: bool,
    pub address_pool: bool,
    pub segment: bool,
    pub resource_group: Option<String>,
}

#[derive(Clone, Debug)]
pub(super) struct Allocation {
    pub token: Uuid,
    pub name: String,
    pub bandwidth: i64,
    pub internet_charge_type: String,
    pub isp: String,
    pub netmode: String,
    pub resource_group: Option<String>,
}
impl Allocation {
    pub fn bandwidth(eip_bandwidth: i64, package: Option<&str>) -> i64 {
        if package.is_some() {
            PACKAGE_ALLOCATION_MBPS
        } else {
            eip_bandwidth
        }
    }
    /// A package member may report the package's own method (such as
    /// `PayByDominantTraffic`), which an EIP cannot be allocated with. Its new
    /// address is billed by traffic only until it joins the same package, as in
    /// the operator script this flow replaces.
    pub fn charge(eip_charge: &str, package: Option<&str>) -> String {
        if package.is_some() {
            "PayByTraffic".into()
        } else {
            eip_charge.into()
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Tally {
    pub total: usize,
    pub reached: usize,
}
impl Tally {
    /// Probes inside Alibaba's own network reach its addresses regardless of the
    /// border filter, so they cannot show whether an address is usable.
    pub fn from_rows(rows: &[ProbeResult]) -> Self {
        let independent = rows.iter().filter(|row| !row.network.contains("Alibaba"));
        Self {
            total: independent.clone().count(),
            reached: independent.filter(|row| row.reached).count(),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub(super) struct Plan {
    pub limit: u32,
    pub packets: u32,
    pub minimum: usize,
    pub percent: usize,
}

pub(super) trait Ops {
    async fn instance(&self) -> Result<Instance, Failure>;
    async fn eip(&self, id: &str) -> Result<Option<Eip>, Failure>;
    async fn allocate(&self, request: &Allocation) -> Result<(String, String), Failure>;
    async fn join(&self, package: &str, id: &str) -> Result<(), Failure>;
    async fn leave(&self, package: &str, id: &str) -> Result<(), Failure>;
    async fn associate(&self, id: &str) -> Result<(), Failure>;
    async fn unassociate(&self, id: &str) -> Result<(), Failure>;
    async fn release(&self, id: &str) -> Result<(), Failure>;
}

pub(super) trait Probe {
    async fn reachable(&self, ip: &str, port: u16) -> bool;
    async fn start(&self, ip: &str, port: u16, plan: Plan) -> Result<String, Failure>;
    async fn read(&self, id: &str) -> Result<Option<Tally>, Failure>;
}

pub(super) struct LiveOps<'a> {
    cloud: &'a Cloud,
    account: Account,
    region: String,
    instance: String,
}
impl<'a> LiveOps<'a> {
    pub async fn new(
        cloud: &'a Cloud,
        account: &Account,
        resource: &Resource,
    ) -> Result<Self, Failure> {
        Ok(Self {
            cloud,
            account: cloud.resolved_account(account).await?,
            region: resource.region.clone(),
            instance: resource.cloud_id.clone(),
        })
    }
    async fn vpc(&self, action: &str, mut params: Vec<(&str, String)>) -> Result<Value, Failure> {
        params.push(("RegionId", self.region.clone()));
        self.cloud
            .vpc
            .call(
                &self.account.access_key_id,
                &self.account.access_key_secret,
                action,
                &params,
            )
            .await
    }
    fn binding(&self, id: &str) -> Vec<(&'static str, String)> {
        vec![
            ("AllocationId", id.into()),
            ("InstanceId", self.instance.clone()),
            ("InstanceType", "EcsInstance".into()),
        ]
    }
}
impl Ops for LiveOps<'_> {
    async fn instance(&self) -> Result<Instance, Failure> {
        let value = self
            .cloud
            .ecs
            .call(
                &self.account.access_key_id,
                &self.account.access_key_secret,
                "DescribeInstances",
                &[
                    ("RegionId", self.region.clone()),
                    ("InstanceIds", json!([self.instance]).to_string()),
                    ("PageSize", "10".into()),
                ],
            )
            .await?;
        parse_instance(&value, &self.instance, &self.region)
    }
    async fn eip(&self, id: &str) -> Result<Option<Eip>, Failure> {
        let value = self
            .vpc(
                "DescribeEipAddresses",
                vec![
                    ("AllocationId", id.into()),
                    ("PageNumber", "1".into()),
                    ("PageSize", "10".into()),
                ],
            )
            .await?;
        parse_eip(&value, id)
    }
    async fn allocate(&self, request: &Allocation) -> Result<(String, String), Failure> {
        let mut params = vec![
            ("Bandwidth", request.bandwidth.to_string()),
            ("InternetChargeType", request.internet_charge_type.clone()),
            ("InstanceChargeType", "PostPaid".to_owned()),
            ("ISP", request.isp.clone()),
            ("Netmode", request.netmode.clone()),
            ("Name", request.name.clone()),
            ("Description", "APSP public IP rotation".to_owned()),
            ("ClientToken", request.token.to_string()),
        ];
        if let Some(group) = &request.resource_group {
            params.push(("ResourceGroupId", group.clone()));
        }
        let value = self.vpc("AllocateEipAddress", params).await?;
        let id = value["AllocationId"]
            .as_str()
            .filter(|id| identifier(id, "eip-"))
            .ok_or(Failure::from("invalid_response"))?;
        let ip = value["EipAddress"]
            .as_str()
            .filter(|ip| ip.parse::<std::net::Ipv4Addr>().is_ok())
            .ok_or(Failure::from("invalid_response"))?;
        Ok((id.to_owned(), ip.to_owned()))
    }
    async fn join(&self, package: &str, id: &str) -> Result<(), Failure> {
        self.vpc(
            "AddCommonBandwidthPackageIp",
            vec![
                ("BandwidthPackageId", package.into()),
                ("IpInstanceId", id.into()),
            ],
        )
        .await
        .map(drop)
    }
    async fn leave(&self, package: &str, id: &str) -> Result<(), Failure> {
        self.vpc(
            "RemoveCommonBandwidthPackageIp",
            vec![
                ("BandwidthPackageId", package.into()),
                ("IpInstanceId", id.into()),
            ],
        )
        .await
        .map(drop)
    }
    async fn associate(&self, id: &str) -> Result<(), Failure> {
        self.vpc("AssociateEipAddress", self.binding(id))
            .await
            .map(drop)
    }
    async fn unassociate(&self, id: &str) -> Result<(), Failure> {
        self.vpc("UnassociateEipAddress", self.binding(id))
            .await
            .map(drop)
    }
    async fn release(&self, id: &str) -> Result<(), Failure> {
        self.vpc("ReleaseEipAddress", vec![("AllocationId", id.into())])
            .await
            .map(drop)
    }
}

pub(super) struct LiveProbe {
    service: Globalping,
}
impl LiveProbe {
    pub fn new() -> Result<Self, Failure> {
        Ok(Self {
            service: Globalping::new()?,
        })
    }
}
impl Probe for LiveProbe {
    async fn reachable(&self, ip: &str, port: u16) -> bool {
        let connect = tokio::net::TcpStream::connect((ip, port));
        matches!(
            tokio::time::timeout(Duration::from_secs(5), connect).await,
            Ok(Ok(_))
        )
    }
    async fn start(&self, ip: &str, port: u16, plan: Plan) -> Result<String, Failure> {
        self.service
            .tcp_ping(ip, port, PROBE_COUNTRY, plan.limit, plan.packets)
            .await
    }
    async fn read(&self, id: &str) -> Result<Option<Tally>, Failure> {
        Ok(self
            .service
            .read(id)
            .await?
            .map(|rows| Tally::from_rows(&rows)))
    }
}

fn blank(value: &Value) -> bool {
    value.is_null() || value == ""
}
fn optional(value: &Value) -> Option<String> {
    value
        .as_str()
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
}
fn required(row: &Value, field: &str) -> Result<String, Failure> {
    optional(&row[field])
        .filter(|text| text.len() <= 256 && !text.chars().any(char::is_control))
        .ok_or(Failure::from("invalid_response"))
}

fn parse_instance(value: &Value, id: &str, region: &str) -> Result<Instance, Failure> {
    let entries = value["Instances"]["Instance"]
        .as_array()
        .filter(|entries| entries.len() == 1)
        .ok_or(Failure::from("resource_not_found"))?;
    let row = &entries[0];
    if row["InstanceId"] != id || row["RegionId"] != region {
        return Err("resource_not_found".into());
    }
    let attached_eip = optional(&row["EipAddress"]["AllocationId"]);
    if attached_eip
        .as_deref()
        .is_some_and(|eip| !identifier(eip, "eip-"))
    {
        return Err("invalid_response".into());
    }
    Ok(Instance {
        status: required(row, "Status")?,
        attached_eip,
    })
}

fn parse_eip(value: &Value, id: &str) -> Result<Option<Eip>, Failure> {
    let entries = value["EipAddresses"]["EipAddress"]
        .as_array()
        .ok_or(Failure::from("invalid_response"))?;
    let Some(row) = entries.first() else {
        return Ok(None);
    };
    if entries.len() != 1 || row["AllocationId"] != id {
        return Err("invalid_response".into());
    }
    let ip = required(row, "IpAddress")?;
    if ip.parse::<std::net::Ipv4Addr>().is_err() {
        return Err("invalid_response".into());
    }
    // `Bandwidth` reports the package peak once joined; `EipBandwidth` keeps the EIP's own.
    let bandwidth = [&row["EipBandwidth"], &row["Bandwidth"]]
        .into_iter()
        .find_map(|value| value.as_str().and_then(|text| text.parse::<i64>().ok()))
        .filter(|value| (0..=100_000).contains(value))
        .ok_or(Failure::from("invalid_response"))?;
    let protected = row["SecurityProtectionTypes"]["SecurityProtectionType"]
        .as_array()
        .is_some_and(|types| !types.is_empty());
    Ok(Some(Eip {
        allocation_id: id.to_owned(),
        ip,
        status: required(row, "Status")?,
        instance_id: optional(&row["InstanceId"]),
        package_id: optional(&row["BandwidthPackageId"]),
        isp: required(row, "ISP")?,
        netmode: required(row, "Netmode")?,
        internet_charge_type: required(row, "InternetChargeType")?,
        charge_type: required(row, "ChargeType")?,
        bandwidth,
        mode: required(row, "Mode")?,
        business_status: required(row, "BusinessStatus")?,
        deletion_protection: row["DeletionProtection"] == true,
        security_protection: protected,
        address_pool: !blank(&row["PublicIpAddressPoolId"]),
        segment: !blank(&row["SegmentInstanceId"]),
        resource_group: optional(&row["ResourceGroupId"]),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row() -> Value {
        json!({"AllocationId":"eip-testonly","IpAddress":"192.0.2.1","Status":"InUse","InstanceId":"i-testonly","BandwidthPackageId":"cbwp-testonly","ISP":"BGP","Netmode":"public","InternetChargeType":"PayByTraffic","ChargeType":"PostPaid","Bandwidth":"500","EipBandwidth":"200","Mode":"NAT","BusinessStatus":"Normal","DeletionProtection":false,"SecurityProtectionTypes":{"SecurityProtectionType":[]},"SegmentInstanceId":"","ResourceGroupId":"rg-testonly"})
    }

    #[test]
    fn released_addresses_read_as_absent_and_shared_package_peaks_are_not_the_eip_bandwidth() {
        assert_eq!(
            parse_eip(&json!({"EipAddresses":{"EipAddress":[]}}), "eip-testonly").unwrap(),
            None
        );
        let eip = parse_eip(
            &json!({"EipAddresses":{"EipAddress":[row()]}}),
            "eip-testonly",
        )
        .unwrap()
        .unwrap();
        assert_eq!(eip.bandwidth, 200);
        assert_eq!(eip.package_id.as_deref(), Some("cbwp-testonly"));
        assert!(!eip.security_protection && !eip.address_pool && !eip.segment);
    }

    #[test]
    fn responses_about_another_address_are_refused() {
        assert!(parse_eip(&json!({"EipAddresses":{"EipAddress":[row()]}}), "eip-other").is_err());
        let mut invalid = row();
        invalid["IpAddress"] = "not-an-address".into();
        assert!(
            parse_eip(
                &json!({"EipAddresses":{"EipAddress":[invalid]}}),
                "eip-testonly"
            )
            .is_err()
        );
    }

    #[test]
    fn tally_ignores_probes_inside_the_provider_network() {
        let rows = [
            ("Hangzhou Alibaba Advertising", true),
            ("China Telecom", false),
            ("China Unicom", true),
        ]
        .map(|(network, reached)| ProbeResult {
            network: network.into(),
            reached,
        });
        assert_eq!(
            Tally::from_rows(&rows),
            Tally {
                total: 2,
                reached: 1
            }
        );
    }
}
