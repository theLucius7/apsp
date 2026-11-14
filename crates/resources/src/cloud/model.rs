use crate::error::{ApiError, ApiResult};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, types::Json};
use uuid::Uuid;

/// A registered cloud account. Its access keys live in the credential store; costs keeps the
/// account's bills, balance and CDT readings and operations its automatic bandwidth
/// reduction in their own tables (ADR 0137 steps 6 and 7).
#[derive(Clone, Serialize, FromRow)]
pub struct Account {
    pub id: Uuid,
    pub name: String,
    pub site: String,
    pub credential_id: Option<Uuid>,
    pub enabled: bool,
    pub revision: i64,
    pub next_run_at: i64,
}

/// A registered cloud resource: what it is, what the cloud last reported about it and its
/// IPv6 addresses. Costs keeps its instance bill and operations its automation (bandwidth
/// reduction, power policy, holds and CDT guard) in their own tables (ADR 0137 steps 6
/// and 7).
#[derive(Serialize, FromRow)]
pub struct Resource {
    pub id: Uuid,
    pub account_id: Uuid,
    pub name: String,
    pub kind: String,
    pub region: String,
    pub cloud_id: String,
    pub revision: i64,
    pub snapshot: Option<Json<Snapshot>>,
    pub checked_at: Option<i64>,
    pub error_code: Option<String>,
    pub ipv6_addresses: Json<Vec<String>>,
    pub ipv6_checked_at: Option<i64>,
    pub ipv6_error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Target {
    pub bandwidth_mbps: i64,
    pub charge_type: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snapshot {
    pub kind: String,
    pub cloud_id: String,
    pub region: String,
    pub public_ip: String,
    pub bandwidth_mbps: i64,
    pub charge_type: String,
    pub resource_charge_type: String,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attached_eip_id: Option<String>,
}

impl Target {
    pub fn validate(&self, before: &Snapshot) -> ApiResult<()> {
        if before.attached_eip_id.is_some() {
            return Err(ApiError::BadRequest(
                message("attached_eip_read_only").into(),
            ));
        }
        if !(1..=100).contains(&self.bandwidth_mbps)
            || !matches!(self.charge_type.as_str(), "PayByTraffic" | "PayByBandwidth")
        {
            return Err(ApiError::BadRequest(
                "带宽范围为 1–100 Mbps，请选择有效计费方式".into(),
            ));
        }
        if before.kind == "eip" && self.charge_type != before.charge_type {
            return Err(ApiError::BadRequest(
                "EIP 计费转换请前往阿里云控制台；此接口只调整带宽".into(),
            ));
        }
        Ok(())
    }
    pub fn matches(&self, snapshot: &Snapshot) -> bool {
        self.bandwidth_mbps == snapshot.bandwidth_mbps && self.charge_type == snapshot.charge_type
    }
}

#[derive(Serialize, FromRow)]
pub struct Operation {
    pub id: Uuid,
    pub resource_id: Uuid,
    pub account_revision: i64,
    pub resource_revision: i64,
    pub before_state: Json<Snapshot>,
    pub target: Json<Target>,
    pub source: String,
    pub billing_cycle: Option<String>,
    pub status: String,
    pub created_at: i64,
    pub expires_at: i64,
    pub updated_at: i64,
    pub next_check_at: i64,
    pub error_code: Option<String>,
    pub request_id: Option<String>,
}

pub fn label(value: &str) -> ApiResult<String> {
    crate::text::Text::chars(128)
        .not_empty()
        .trimmed(value)
        .map(str::to_owned)
        .ok_or_else(|| ApiError::BadRequest("名称须为 1–128 个字符".into()))
}

pub fn identifier(value: &str, prefix: &str) -> bool {
    value.starts_with(prefix)
        && (3..=80).contains(&value.len())
        && value
            .bytes()
            .all(|v| v.is_ascii_lowercase() || v.is_ascii_digit() || v == b'-')
}

pub fn message(code: &str) -> &'static str {
    match code {
        "cdt_state_mismatch" => "CDT 历史状态与账号、资源或账期不符，守护已暂停操作",
        "cdt_reset_pending" => "已进入新账期，等待 CDT 用量重置后恢复守护",
        "cdt_month_changed" => "查询跨越月界，等待下一轮重新读取 CDT",
        "cdt_not_enabled" => "零用量无法确认已启用 CDT 公网计费，保持实例原状",
        "cdt_unsupported_isp" => "额度池存在非 BGP 流量，请核对适用范围",
        "authentication_failed" => "访问密钥无效或缺少权限",
        "credential_unavailable" => "集中云凭据已停用、用途不符或解密密钥缺失，请在凭据中心核对",
        "credential_invalid" => {
            "集中云凭据字段或提供方不符，需要阿里云 access_key_id 与 access_key_secret"
        }
        "capacity_unavailable" => "实例库存或抢占价格条件不足，保活冷却后再尝试",
        "insufficient_balance" => "账号余额不足或资源欠费，请前往阿里云核对",
        "resource_locked" => "实例已被云端锁定，暂不能启停",
        "request_rejected" => "云端明确拒绝本次启停请求，请核对状态与权限",
        "stop_mode_unsupported" => "节省停机需要按量付费 VPC 实例",
        "stop_mode_mismatch" => "实例已停止，但实际停机模式与请求不符；请在云端核对收费",
        "stop_mode_unknown" => "实例已停止，但云端未提供可核实的停机模式",
        "rate_limited" => "云服务限流，请稍后重试",
        "resource_not_found" => "未找到指定地域和标识的资源",
        "attached_eip_read_only" => "实例公网来自绑定 EIP，此处只读；请在 EIP 控制台管理带宽与计费",
        "unsupported_resource" => "仅支持 ECS 固定公网 IP 和按量付费的独立 EIP；不操作共享带宽包",
        "resource_busy" => "云资源正在变更或状态不支持调整",
        "billing_incomplete" => "账单分页、单位或响应不完整，自动控制暂停",
        "state_changed" => "云资源或本地配置已变化，请重新预览",
        "policy_inactive" => "策略已关闭或当月账单不可用于自动控制",
        "request_timeout" | "network_error" | "response_error" => {
            "云端响应未确认，请核对结果后再操作"
        }
        "invalid_response" => "云服务响应与预期不符",
        "provider_rejected" => "云服务未接受请求，请检查资源限制与权限",
        "awaiting_confirmation" => "请求结果尚未核实，系统只读回状态，不重复提交",
        "all_candidates_blocked" => "候选地址从国内都连不上，已放弃本次更换",
        "service_unreachable" => "新地址绑定后面板连不上探测端口，无法判断可用性",
        "probe_unavailable" => "国内探测服务暂不可用，无法判断新地址是否可用",
        "probe_inconclusive" => "国内探测结果不足，无法判断新地址是否可用",
        "old_eip_retained" => "旧 EIP 未能释放，请到阿里云控制台手动释放，否则闲置会产生费用",
        _ => "云接口暂时不可用",
    }
}
