use crate::error::{ApiError, ApiResult};
pub use crate::{digest::json as digest, text::label};
use apsp_protocol::fleet::Operation;
use apsp_traffic::runtime_workflows::automation::runtime_step_policy;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use uuid::Uuid;

#[derive(Clone, Debug, Serialize)]
pub struct Step {
    pub kind: String,
    pub service: Option<String>,
    pub timeout_secs: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backup_schedule_id: Option<Uuid>,
    /// True only when a persisted historical row carried the removed `runtime_version` key.
    #[serde(skip)]
    legacy_runtime_version: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WireStep {
    kind: String,
    service: Option<String>,
    timeout_secs: u32,
    #[serde(
        default,
        rename = "runtime_version",
        deserialize_with = "deserialize_legacy_runtime_version"
    )]
    legacy_runtime_version: bool,
    #[serde(default)]
    backup_schedule_id: Option<Uuid>,
}

fn deserialize_legacy_runtime_version<'de, D>(deserializer: D) -> Result<bool, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let _ = Option::<String>::deserialize(deserializer)?;
    Ok(true)
}

impl<'de> Deserialize<'de> for Step {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let wire = WireStep::deserialize(deserializer)?;
        Ok(Self {
            kind: wire.kind,
            service: wire.service,
            timeout_secs: wire.timeout_secs,
            backup_schedule_id: wire.backup_schedule_id,
            legacy_runtime_version: wire.legacy_runtime_version,
        })
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Plan {
    pub name: String,
    pub steps: Vec<Step>,
    pub batch_size: usize,
    pub concurrency: usize,
    pub pause_between_batches: bool,
    pub max_duration_secs: i64,
}

impl Step {
    pub fn new(
        kind: impl Into<String>,
        service: Option<String>,
        timeout_secs: u32,
        backup_schedule_id: Option<Uuid>,
    ) -> Self {
        Self {
            kind: kind.into(),
            service,
            timeout_secs,
            backup_schedule_id,
            legacy_runtime_version: false,
        }
    }

    pub(super) fn has_legacy_runtime_version(&self) -> bool {
        self.legacy_runtime_version
    }

    pub fn is_runtime(&self) -> bool {
        runtime_step_policy(&self.kind).is_some()
    }
    pub fn is_panel(&self) -> bool {
        self.kind == "panel_backup"
    }
    pub fn is_fleet(&self) -> bool {
        !self.is_runtime() && !self.is_panel()
    }

    pub fn validate(&self) -> ApiResult<()> {
        if self.legacy_runtime_version {
            return Err(ApiError::BadRequest(
                "部署恢复步骤不再接受独立运行时版本参数，请移除 runtime_version 后重新建立方案"
                    .into(),
            ));
        }
        if self.is_panel() {
            if self.service.is_some()
                || self.backup_schedule_id.is_none()
                || !(60..=1800).contains(&self.timeout_secs)
            {
                return Err(ApiError::BadRequest(
                    "完整面板备份须选择已登记备份计划、60–1800秒预算，且不能附加服务参数".into(),
                ));
            }
            return Ok(());
        }
        if runtime_step_policy(&self.kind).is_some() {
            if self.service.is_some()
                || self.backup_schedule_id.is_some()
                || !(1..=600).contains(&self.timeout_secs)
            {
                return Err(ApiError::BadRequest(
                    "固定失败配置恢复步骤须使用 1–600 秒预算，且不能附加服务或备份参数".into(),
                ));
            }
            return Ok(());
        }
        if self.backup_schedule_id.is_some() {
            return Err(ApiError::BadRequest(
                "此服务或盘点步骤不能附加其他执行器参数".into(),
            ));
        }
        self.operation().map(|_| ())
    }

    pub fn operation(&self) -> ApiResult<Operation> {
        if !(1..=600).contains(&self.timeout_secs) {
            return Err(ApiError::BadRequest("步骤超时须为 1–600 秒".into()));
        }
        let action = match self.kind.as_str() {
            "system_snapshot" if self.service.is_none() => {
                return Ok(Operation::Snapshot {});
            }
            "service_status" => "status",
            "service_start" => "start",
            "service_stop" => "stop",
            "service_restart" => "restart",
            _ => return Err(ApiError::BadRequest("不支持的固定操作类型".into())),
        };
        let service = self.service.as_deref().unwrap_or("");
        if service.is_empty()
            || service.len() > 128
            || service.starts_with('-')
            || !service
                .bytes()
                .all(|v| v.is_ascii_alphanumeric() || matches!(v, b'@' | b'_' | b'.' | b'-'))
        {
            return Err(ApiError::BadRequest("服务名无效".into()));
        }
        Ok(Operation::Service {
            unit: service.into(),
            action: action.into(),
        })
    }

    pub fn permission(&self) -> &'static str {
        if self.is_panel() {
            "recovery:write"
        } else if let Some(policy) = runtime_step_policy(&self.kind) {
            policy.capability
        } else if self.kind == "system_snapshot" {
            "operations:read"
        } else if self.kind == "service_status" {
            "services:read"
        } else {
            "services:write"
        }
    }
}

impl Plan {
    pub(super) fn has_legacy_runtime_version(&self) -> bool {
        self.steps.iter().any(Step::has_legacy_runtime_version)
    }

    pub fn validate(&self, targets: &[i64]) -> ApiResult<()> {
        label(&self.name, 128)?;
        if !(1..=256).contains(&targets.len())
            || targets.iter().any(|v| *v <= 0)
            || targets.iter().collect::<BTreeSet<_>>().len() != targets.len()
            || !(1..=16).contains(&self.steps.len())
            || !(1..=32).contains(&self.batch_size)
            || !(1..=16).contains(&self.concurrency)
            || self.concurrency > self.batch_size
            || !(60..=86400).contains(&self.max_duration_secs)
        {
            return Err(ApiError::BadRequest(
                "目标、步骤、并发、批次或总时长无效".into(),
            ));
        }
        for step in &self.steps {
            step.validate()?;
        }
        if self
            .steps
            .iter()
            .enumerate()
            .any(|(position, step)| step.is_panel() && position != 0)
        {
            return Err(ApiError::BadRequest(
                "完整面板备份只可作为唯一首步，同一任务生成一次恢复点，随后再执行首台及后续批次"
                    .into(),
            ));
        }
        Ok(())
    }
}

pub fn batch(index: usize, size: usize) -> i32 {
    if index == 0 {
        0
    } else {
        (1 + (index - 1) / size) as i32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_parameters_cannot_introduce_shell_or_option_syntax() {
        for service in ["--help", "foo;id", "$(id)", "../foo", "foo\nbar", "foo'bar"] {
            assert!(
                Step::new("service_restart", Some(service.into()), 30, None)
                    .operation()
                    .is_err()
            );
        }
        assert!(matches!(
            Step::new("service_status", Some("demo@node.service".into()), 30, None)
                .operation()
                .unwrap(),
            Operation::Service { unit, action } if unit == "demo@node.service" && action == "status"
        ));
    }

    #[test]
    fn canary_is_separate_from_fixed_following_batches() {
        assert_eq!(
            (0..8).map(|v| batch(v, 3)).collect::<Vec<_>>(),
            vec![0, 1, 1, 1, 2, 2, 2, 3]
        );
    }

    #[test]
    fn runtime_step_omits_runtime_version_and_rejects_legacy_version_inputs() {
        let mut step: Step = serde_json::from_value(serde_json::json!({
            "kind": "singbox_retry_deployment",
            "service": null,
            "timeout_secs": 300
        }))
        .unwrap();
        assert!(step.is_runtime());
        assert!(step.validate().is_ok());
        assert_eq!(step.permission(), "proxy:write");
        let serialized = serde_json::to_value(&step).unwrap();
        assert!(serialized.get("runtime_version").is_none());

        step.service = Some("other.service".into());
        assert!(step.validate().is_err());

        for legacy in [
            serde_json::json!({
                "kind": "singbox_retry_deployment",
                "service": null,
                "timeout_secs": 300,
                "runtime_version": "1.14.4-apsp.2"
            }),
            serde_json::json!({
                "kind": "singbox_retry_deployment",
                "service": null,
                "timeout_secs": 300,
                "runtime_version": null
            }),
        ] {
            let historical: Step = serde_json::from_value(legacy).unwrap();
            assert!(historical.is_runtime());
            assert!(historical.has_legacy_runtime_version());
            assert!(historical.validate().is_err());
        }

        assert!(
            serde_json::from_value::<Step>(serde_json::json!({
                "kind": "singbox_retry_deployment",
                "service": null,
                "timeout_secs": 300,
                "unexpected": true
            }))
            .is_err()
        );
    }

    #[test]
    fn executor_parameters_and_backup_barrier_are_explicit() {
        let legacy: Step = serde_json::from_value(
            serde_json::json!({"kind":"system_snapshot","service":null,"timeout_secs":30}),
        )
        .unwrap();
        assert!(legacy.validate().is_ok());
        let backup = Step::new("panel_backup", None, 600, Some(Uuid::new_v4()));
        assert!(backup.validate().is_ok());
        assert!(backup.operation().is_err());
        let mut plan = Plan {
            name: "TEST_ONLY deployment recovery point".into(),
            steps: vec![backup.clone(), legacy.clone()],
            batch_size: 2,
            concurrency: 1,
            pause_between_batches: true,
            max_duration_secs: 1800,
        };
        assert!(plan.validate(&[1, 2]).is_ok());
        plan.steps = vec![legacy, backup];
        assert!(plan.validate(&[1, 2]).is_err());
    }
}
