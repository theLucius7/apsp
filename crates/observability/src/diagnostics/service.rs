use super::*;
use crate::diagnostic_tools;
use apsp_governance::releases::{VerifiedRunningComponent, verify_running_component};
use apsp_protocol::{
    DIAGNOSTIC_SERVICE_CAPABILITY, DiagnosticResourceBudget,
    bundled::{valid_segment, valid_sha256, valid_version_segment},
    platform::ARTIFACT_TARGETS,
    release::RELEASE_SOURCE_REPO,
};
use std::{collections::BTreeMap, future::Future, pin::Pin};

pub type PlanFuture<'a> = Pin<Box<dyn Future<Output = ApiResult<JobPlan>> + Send + 'a>>;
pub type SectionFuture<'a> = Pin<Box<dyn Future<Output = ApiResult<()>> + Send + 'a>>;

/// Verified signed Agent package and embedded diagnostic component identity frozen at dispatch.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FrozenComponentIdentity {
    pub schema: u32,
    pub agent_version: String,
    pub release_tag: String,
    pub target: String,
    pub source_repo: String,
    pub agent_sha256: String,
    pub agent_bytes: u64,
    pub component: String,
    pub component_version: String,
    pub component_sha256: String,
}

impl From<VerifiedRunningComponent> for FrozenComponentIdentity {
    fn from(verified: VerifiedRunningComponent) -> Self {
        Self {
            schema: 1,
            agent_version: verified.agent_version,
            release_tag: verified.release_tag,
            target: verified.target,
            source_repo: verified.source_repo,
            agent_sha256: verified.agent_sha256,
            agent_bytes: verified.agent_bytes,
            component: verified.component,
            component_version: verified.component_version,
            component_sha256: verified.component_sha256,
        }
    }
}

impl FrozenComponentIdentity {
    pub fn valid(&self) -> bool {
        self.schema == 1
            && !self.agent_version.is_empty()
            && self.release_tag == format!("agent-v{}", self.agent_version)
            && ARTIFACT_TARGETS.contains(&self.target.as_str())
            && self.source_repo == RELEASE_SOURCE_REPO
            && valid_sha256(&self.agent_sha256)
            && self.agent_bytes > 0
            && valid_segment(&self.component)
            && valid_version_segment(&self.component_version)
            && valid_sha256(&self.component_sha256)
    }

    pub fn from_job(job: &Value) -> ApiResult<Option<Self>> {
        let Some(raw) = job.get("agent_bundle") else {
            return Ok(None);
        };
        if job
            .get("artifact")
            .is_some_and(|artifact| !artifact.is_null())
        {
            return Err(ApiError::Conflict(
                "诊断任务不能同时携带历史证明与当前 Agent 发布内置组件证明".into(),
            ));
        }
        let frozen: Self = serde_json::from_value(raw.clone())
            .map_err(|_| ApiError::Conflict("诊断任务冻结的签名组件身份格式无效".into()))?;
        let declared_tool = match (job["plugin"].as_str(), job["tool"].as_str()) {
            (Some(plugin), None) => Some(plugin),
            (None, Some(tool)) => Some(tool),
            (Some(plugin), Some(tool)) if plugin == tool => Some(plugin),
            _ => None,
        };
        if !frozen.valid()
            || declared_tool != Some(frozen.component.as_str())
            || job["version"].as_str() != Some(frozen.component_version.as_str())
        {
            return Err(ApiError::Conflict(
                "诊断任务冻结的签名组件身份与任务声明不一致".into(),
            ));
        }
        Ok(Some(frozen))
    }

    pub fn verify_live(&self, row: &sqlx::postgres::PgRow, now: i64) -> ApiResult<()> {
        if !self.valid() {
            return Err(ApiError::Conflict("诊断任务冻结的签名组件身份无效".into()));
        }
        let last_seen: Option<i64> = row.get("last_seen");
        if !last_seen.is_some_and(|seen| seen <= now + 60 && now.saturating_sub(seen) <= 60) {
            return Err(ApiError::Conflict(
                "Agent 当前离线，无法核实运行中的已验签 Agent 发布身份".into(),
            ));
        }
        let received_at: Option<i64> = row.get("static_info_received_at");
        if !received_at.is_some_and(|at| at > 0 && at <= now + 60 && now.saturating_sub(at) <= 600)
        {
            return Err(ApiError::Conflict(
                "尚未收到当前会话的新鲜设备组件身份上报，请等待设备同步".into(),
            ));
        }
        let static_info: Value = row.get("static_info");
        if static_info["agent_version"].as_str() != Some(self.agent_version.as_str())
            || apsp_governance::releases::reported_agent_sha256(&static_info)
                != Some(self.agent_sha256.as_str())
            || static_info["components"][&self.component].as_str()
                != Some(self.component_version.as_str())
            || static_info["component_digests"][&self.component].as_str()
                != Some(self.component_sha256.as_str())
            || !apsp_governance::releases::agent_targets(&static_info).contains(&self.target)
        {
            return Err(ApiError::Conflict(
                "设备当前运行的签名 Agent 或内置组件身份与派单冻结值不一致".into(),
            ));
        }
        Ok(())
    }
}

pub struct SectionContext<'a> {
    pub server_id: i64,
    pub job: &'a Value,
    pub created_at: i64,
    pub expires_at: i64,
    pub job_generation: i64,
    pub live_verified: bool,
}

pub struct JobPlan {
    pub timeout_secs: u64,
    pub budget: DiagnosticResourceBudget,
    pub options: BTreeMap<String, String>,
    pub expected_sections: Vec<String>,
    pub metadata: BTreeMap<String, Value>,
}

/// Diagnostic tools translate parameters and parse reports; the service owns task state.
pub trait DiagnosticTool: Send + Sync {
    fn id(&self) -> &'static str;
    fn version(&self) -> &'static str;
    fn title(&self) -> &'static str;
    fn required_capabilities(&self) -> &'static [&'static str];
    fn required_os(&self) -> Option<&'static str> {
        Some("linux")
    }
    fn expected_version(&self, _static_info: &Value) -> ApiResult<String> {
        Ok(self.version().to_owned())
    }
    fn freeze_dispatch(
        &self,
        _verified: &VerifiedRunningComponent,
        _plan: &mut JobPlan,
    ) -> ApiResult<()> {
        Ok(())
    }
    fn start_denial(&self, _job: &Value) -> Option<&'static str> {
        None
    }
    fn can_dispatch(&self, _job: &Value, _capabilities: &Value) -> bool {
        true
    }
    fn plan<'a>(
        &'a self,
        request: Value,
        server_id: i64,
        connection: &'a mut sqlx::PgConnection,
    ) -> PlanFuture<'a>;
    fn report_url_allowed(&self, _value: &str) -> bool {
        false
    }
    /// Parse and project an accepted chapter inside the service's transaction.
    fn record_section<'a>(
        &'a self,
        _context: SectionContext<'a>,
        _update: &'a DiagnosticSectionUpdate,
        _connection: &'a mut sqlx::PgConnection,
    ) -> SectionFuture<'a> {
        Box::pin(async { Ok(()) })
    }
}

#[derive(Serialize)]
pub struct ToolReadiness {
    pub tool: &'static str,
    pub title: &'static str,
    pub version: String,
    pub ready: bool,
    pub reason: Option<String>,
}

#[derive(Serialize)]
pub struct DiagnosticView {
    pub cancel_supported: bool,
    pub tools: Vec<ToolReadiness>,
    pub reports: Vec<ReportRecord>,
}

pub fn ready(row: &sqlx::postgres::PgRow, tool: &dyn DiagnosticTool) -> ApiResult<&'static str> {
    let info: Value = row.get("static_info");
    if tool
        .required_os()
        .is_some_and(|os| info["os"].as_str() != Some(os))
    {
        let reason = if tool.required_os() == Some("linux") {
            "当前诊断工具仅支持已识别的 Linux 设备"
        } else {
            "当前诊断工具不支持此设备操作系统"
        };
        return Err(ApiError::Conflict(reason.into()));
    }
    if !row.get::<Option<i64>, _>("last_seen").is_some_and(|seen| {
        seen <= now_timestamp() + 60 && now_timestamp().saturating_sub(seen) <= 60
    }) {
        return Err(ApiError::Conflict(
            "Agent 当前离线，请在设备上线后运行诊断".into(),
        ));
    }
    let capabilities: Value = row.get("capabilities");
    let has_capability = |required: &str| {
        capabilities
            .as_array()
            .is_some_and(|values| values.iter().any(|value| value.as_str() == Some(required)))
    };
    if !has_capability(apsp_protocol::SELF_CONTAINED_AGENT_CAPABILITY) {
        return Err(ApiError::Conflict(
            "请升级至自带诊断能力的当前 Agent 版本".into(),
        ));
    }
    for required in [
        apsp_protocol::DIAGNOSTIC_SECTIONS_CAPABILITY,
        DIAGNOSTIC_SERVICE_CAPABILITY,
        apsp_protocol::DIAGNOSTIC_COMPLETION_CAPABILITY,
        apsp_protocol::DIAGNOSTIC_CPU_CEILING_CAPABILITY,
    ]
    .into_iter()
    .chain(tool.required_capabilities().iter().copied())
    {
        if !has_capability(required) {
            if required == apsp_protocol::DIAGNOSTIC_CPU_CEILING_CAPABILITY {
                return Err(ApiError::Conflict(
                    "此 Agent 尚不支持诊断 CPU 百分比硬上限，请升级并使用支持 systemd/cgroup v2 CPU 控制器的设备".into(),
                ));
            }
            return Err(ApiError::Conflict(
                "此 Agent 尚不支持当前诊断服务，请先升级 Agent".into(),
            ));
        }
    }
    match info["arch"].as_str() {
        Some("aarch64" | "arm64") => Ok("arm64"),
        Some("x86_64" | "amd64") => Ok("amd64"),
        _ => Err(ApiError::Conflict("设备架构未知，无法运行诊断工具".into())),
    }
}

pub(crate) async fn verify_tool_identity(
    state: &AppState,
    row: &sqlx::postgres::PgRow,
    tool: &dyn DiagnosticTool,
    now: i64,
) -> ApiResult<VerifiedRunningComponent> {
    let static_info: Value = row.get("static_info");
    let received_at: Option<i64> = row.get("static_info_received_at");
    let last_seen: Option<i64> = row.get("last_seen");
    let expected_version = tool.expected_version(&static_info)?;
    verify_running_component(
        state,
        &static_info,
        received_at,
        last_seen,
        now,
        tool.id(),
        &expected_version,
    )
    .await
}

pub(crate) async fn accepts_tasks(state: &AppState, server_id: i64) -> ApiResult<()> {
    let mut tx = state.pool.begin().await?;
    crate::device_work::ensure_accepts_tasks_tx(&mut tx, server_id).await
}

pub(crate) async fn readiness(
    state: &AppState,
    server_id: i64,
    row: &sqlx::postgres::PgRow,
    tool: &dyn DiagnosticTool,
) -> ToolReadiness {
    let static_info: Value = row.get("static_info");
    let version = tool
        .expected_version(&static_info)
        .unwrap_or_else(|_| tool.version().to_owned());
    let result = async {
        ready(row, tool)?;
        verify_tool_identity(state, row, tool, now_timestamp()).await?;
        accepts_tasks(state, server_id).await?;
        Ok::<(), ApiError>(())
    }
    .await;
    ToolReadiness {
        tool: tool.id(),
        title: tool.title(),
        version,
        ready: result.is_ok(),
        reason: result.err().map(|error| error.to_string()),
    }
}

pub async fn history(
    state: &AppState,
    server_id: i64,
    tool: Option<&str>,
) -> ApiResult<Vec<ReportRecord>> {
    reject_queued(&mut *state.pool.acquire().await?, server_id).await?;
    let query = HISTORY_QUERY.replace(
        "WHERE j.server_id=$1",
        "WHERE j.server_id=$1 AND ($2::text IS NULL OR j.job->>'plugin'=$2 OR ($2='nodequality' AND j.job->>'plugin' IS NULL))",
    );
    Ok(sqlx::query_as(&query)
        .bind(server_id)
        .bind(tool)
        .fetch_all(&state.pool)
        .await?)
}

pub async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> ApiResult<Json<DiagnosticView>> {
    auth::require_admin(&state, &headers).await?;
    expire(&state).await?;
    let row = sqlx::query(
        "SELECT static_info,static_info_received_at,last_seen,capabilities FROM servers WHERE id=$1 AND deleted_at IS NULL",
    )
    .bind(id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or(ApiError::NotFound)?;
    let mut tools = Vec::new();
    for tool in diagnostic_tools::all() {
        tools.push(readiness(&state, id, &row, *tool).await);
    }
    Ok(Json(DiagnosticView {
        cancel_supported: cancel_supported(&row.get::<Value, _>("capabilities")),
        tools,
        reports: history(&state, id, None).await?,
    }))
}

pub async fn create(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((id, tool)): Path<(i64, String)>,
    Json(request): Json<Value>,
) -> ApiResult<(StatusCode, Json<ReportRecord>)> {
    auth::require_admin(&state, &headers).await?;
    let tool = diagnostic_tools::find(&tool)
        .ok_or_else(|| ApiError::BadRequest("诊断工具没有登记".into()))?;
    Ok((
        StatusCode::CREATED,
        Json(create_job(&state, id, tool, request).await?),
    ))
}

fn valid_component(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
}

fn validate_plan(tool: &dyn DiagnosticTool, version: &str, plan: &JobPlan) -> ApiResult<()> {
    let names = &plan.expected_sections;
    let mut distinct = std::collections::BTreeSet::new();
    let reserved = [
        "id",
        "plugin",
        "version",
        "artifact",
        "timeout_secs",
        "expires_at",
        "options",
        "resource_budget",
        "agent_bundle",
    ];
    if !valid_component(tool.id())
        || !valid_component(version)
        || !(1..=3600).contains(&plan.timeout_secs)
        || !plan.budget.valid()
        || names.is_empty()
        || names.len() > apsp_protocol::DIAGNOSTIC_SECTION_COUNT
        || names.iter().any(|name| {
            name.is_empty()
                || name.len() > 64
                || !name
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
                || !distinct.insert(name)
        })
        || plan.options.len() > 32
        || plan
            .options
            .iter()
            .any(|(key, value)| key.is_empty() || key.len() > 64 || value.len() > 64 * 1024)
        || plan
            .options
            .iter()
            .map(|(key, value)| key.len() + value.len())
            .sum::<usize>()
            > 256 * 1024
        || plan.metadata.len() > 16
        || plan
            .metadata
            .keys()
            .any(|key| key.len() > 64 || reserved.contains(&key.as_str()))
        || serde_json::to_vec(&plan.metadata).map_or(true, |value| value.len() > 64 * 1024)
    {
        return Err(ApiError::BadRequest(
            "诊断计划的预算、参数或报告章节无效".into(),
        ));
    }
    Ok(())
}

pub(crate) async fn create_job(
    state: &AppState,
    id: i64,
    tool: &dyn DiagnosticTool,
    request: Value,
) -> ApiResult<ReportRecord> {
    let mut tx = state.pool.begin().await?;
    crate::device_work::ensure_accepts_tasks_tx(&mut tx, id).await?;
    let row = crate::servers::lock_row(
        &mut tx,
        id,
        crate::servers::RowLock::Update,
        "static_info,static_info_received_at,last_seen,capabilities",
    )
    .await?
    .ok_or(ApiError::NotFound)?;
    ready(&row, tool)?;
    reject_queued(&mut tx, id).await?;
    let now = now_timestamp();
    sqlx::query("UPDATE diagnostic_jobs SET status='failed',error='任务超时或设备未及时回报',updated_at=$2 WHERE server_id=$1 AND status IN ('queued','running') AND expires_at<=$2")
        .bind(id).bind(now).execute(&mut *tx).await?;
    let active: bool = sqlx::query_scalar(UNRESOLVED_QUERY)
        .bind(id)
        .bind(None::<Uuid>)
        .fetch_one(&mut *tx)
        .await?;
    if active {
        return Err(ApiError::Conflict(
            "此服务器已有诊断任务或正在等待清理、取消确认，请等待设备完成".into(),
        ));
    }
    let mut plan = tool.plan(request, id, &mut tx).await?;
    let verified = verify_tool_identity(state, &row, tool, now).await?;
    tool.freeze_dispatch(&verified, &mut plan)?;
    validate_plan(tool, &verified.component_version, &plan)?;
    let frozen = FrozenComponentIdentity::from(verified.clone());
    let job = DiagnosticJob {
        id: Uuid::new_v4(),
        tool: tool.id().into(),
        version: verified.component_version,
        artifact: None,
        timeout_secs: plan.timeout_secs,
        expires_at: Some(now + plan.timeout_secs as i64 + 300),
        resource_budget: Some(plan.budget),
        options: plan.options,
    };
    let mut saved_job = serde_json::to_value(&job).map_err(anyhow::Error::from)?;
    for (key, value) in plan.metadata {
        saved_job[&key] = value;
    }
    saved_job["agent_bundle"] = serde_json::to_value(frozen).map_err(anyhow::Error::from)?;
    sqlx::query("INSERT INTO diagnostic_jobs(id,server_id,job,created_at,updated_at,expires_at,expected_sections) VALUES($1,$2,$3,$4,$4,$5,$6)")
        .bind(job.id).bind(id).bind(saved_job).bind(now).bind(job.expires_at)
        .bind(plan.expected_sections).execute(&mut *tx).await?;
    let record = sqlx::query_as(RECORD_QUERY)
        .bind(job.id)
        .bind(id)
        .fetch_one(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(record)
}

pub const LEGACY_UNFROZEN_DENIAL: &str = "历史未冻结签名 Agent 发布与内置组件身份的诊断任务不再派发执行；已有检查点与历史记录保留只读收据与清理回执";

pub(crate) async fn reject_queued(
    connection: &mut sqlx::PgConnection,
    server_id: i64,
) -> ApiResult<()> {
    let rows = sqlx::query("SELECT id,job FROM diagnostic_jobs WHERE server_id=$1 AND status='queued' ORDER BY created_at,id FOR UPDATE")
        .bind(server_id).fetch_all(&mut *connection).await?;
    for row in rows {
        let job: Value = row.get("job");
        let tool_denial = diagnostic_tools::for_job(&job).and_then(|tool| tool.start_denial(&job));
        let unfrozen = FrozenComponentIdentity::from_job(&job)
            .ok()
            .flatten()
            .is_none()
            .then_some(LEGACY_UNFROZEN_DENIAL);
        if let Some(reason) = tool_denial.or(unfrozen) {
            // A panel queue may lag a durable device start. Keep completion false so
            // an existing checkpoint can still return its late report or cancel.
            sqlx::query("UPDATE diagnostic_jobs SET status='failed',error=$3,updated_at=$4 WHERE id=$1 AND server_id=$2 AND status='queued'")
                .bind(row.get::<Uuid,_>("id")).bind(server_id).bind(reason).bind(now_timestamp())
                .execute(&mut *connection).await?;
        }
    }
    Ok(())
}

pub(super) fn validate_tool_report(job: &Value, report: &DiagnosticReport) -> ApiResult<()> {
    let Some(url) = &report.report_url else {
        // Historical text remains writable even when its tool metadata predates registration.
        return Ok(());
    };
    // Before registration, every diagnostic belonged to the original tool.
    let tool = diagnostic_tools::for_job(job)
        .ok_or_else(|| ApiError::Conflict("任务的诊断工具已不可用".into()))?;
    if !tool.report_url_allowed(url) {
        return Err(ApiError::BadRequest(
            "报告链接不符合已登记工具的允许范围".into(),
        ));
    }
    Ok(())
}

pub fn cancel_supported(capabilities: &Value) -> bool {
    super::cancellation::supported(capabilities)
}
