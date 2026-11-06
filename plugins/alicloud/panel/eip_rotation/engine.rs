use super::{
    Attempt, Before, Candidate, Rotation,
    client::{Allocation, Eip, LiveOps, Ops, Plan, Probe, Tally},
};
use crate::{cloud_api::Failure, error::ApiResult};
use apsp_protocol::now_timestamp;
use sqlx::{PgPool, Postgres, Transaction, types::Json as DbJson};
use uuid::Uuid;

const STALL_SECONDS: i64 = 180;
const SERVING_SECONDS: i64 = 60;
const MAX_RETRY_WAIT: i64 = 30;
pub(super) const SCREEN: Plan = Plan {
    limit: 8,
    packets: 2,
    minimum: 3,
    percent: 85,
};
pub(super) const CONFIRM: Plan = Plan {
    limit: 14,
    packets: 4,
    minimum: 6,
    percent: 90,
};

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Verdict {
    Clean,
    Blocked,
    Inconclusive,
}
pub(super) fn verdict(tally: Tally, plan: Plan) -> Verdict {
    if tally.total < plan.minimum {
        Verdict::Inconclusive
    } else if tally.reached * 100 >= tally.total * plan.percent {
        Verdict::Clean
    } else {
        Verdict::Blocked
    }
}

enum Flow {
    Again,
    Wait(i64),
}
enum Progress {
    Done,
    Pending,
}
const CONFLICT: &str = "state_changed";

fn enter(row: &mut Rotation, phase: &str, now: i64) {
    row.phase = phase.into();
    row.phase_started_at = now;
    row.measurement_id = None;
}
/// Stops replacing addresses and returns the instance to its original EIP.
fn abort(row: &mut Rotation, code: &str, now: i64) {
    row.error_code = Some(code.into());
    row.after_drop = None;
    enter(row, "restore", now);
}
/// Stops all automatic changes; the instance's actual state needs a person.
fn halt(row: &mut Rotation, code: &str) {
    row.status = "unknown".into();
    row.error_code = Some(code.into());
}
fn restoring(row: &Rotation) -> bool {
    row.phase == "restore" || row.after_drop.as_deref() == Some("finish")
}
fn succeed(row: &mut Rotation, warning: Option<&str>) {
    row.status = "succeeded".into();
    row.new_ip = row.candidate.as_ref().and_then(|c| c.ip.clone());
    row.error_code = warning.map(str::to_owned);
}
fn next_attempt(row: &mut Rotation, now: i64) -> Flow {
    if row.attempt >= row.max_attempts {
        abort(row, "all_candidates_blocked", now);
    } else {
        row.attempt += 1;
        row.candidate = None;
        enter(row, "allocate", now);
    }
    Flow::Again
}
fn request(row: &Rotation, before: &Before, token: Uuid) -> Allocation {
    let package = before.package_id.as_deref();
    Allocation {
        token,
        name: format!("apsp-rotated-{}", &row.id.simple().to_string()[..8]),
        bandwidth: Allocation::bandwidth(before.bandwidth, package),
        internet_charge_type: Allocation::charge(&before.internet_charge_type, package),
        isp: before.isp.clone(),
        netmode: before.netmode.clone(),
        resource_group: before.resource_group.clone(),
    }
}
fn candidate_id(row: &Rotation) -> Result<String, Failure> {
    row.candidate
        .as_ref()
        .and_then(|c| c.allocation_id.clone())
        .ok_or(Failure::from(CONFLICT))
}
fn candidate_ip(row: &Rotation) -> Result<String, Failure> {
    row.candidate
        .as_ref()
        .and_then(|c| c.ip.clone())
        .ok_or(Failure::from(CONFLICT))
}
fn record(row: &mut Rotation, change: impl FnOnce(&mut Attempt)) {
    let attempt = row.attempt;
    if let Some(entry) = row.attempts.iter_mut().rev().find(|a| a.n == attempt) {
        change(entry);
    }
}

async fn allocate<O: Ops>(
    row: &mut Rotation,
    before: &Before,
    ops: &O,
    now: i64,
) -> Result<Flow, Failure> {
    let Some(candidate) = row.candidate.as_ref() else {
        // The token is saved before the request, so a lost response repeats the
        // same token and yields the same address rather than a second one.
        row.candidate = Some(DbJson(Candidate {
            token: Uuid::new_v4(),
            allocation_id: None,
            ip: None,
        }));
        return Ok(Flow::Again);
    };
    let token = candidate.token;
    let (id, ip) = ops.allocate(&request(row, before, token)).await?;
    let duplicate = ip == before.public_ip || row.attempts.iter().any(|a| a.ip == ip);
    row.attempts.push(Attempt {
        n: row.attempt,
        allocation_id: id.clone(),
        ip: ip.clone(),
        verdict: if duplicate { "duplicate" } else { "pending" }.into(),
        screen: None,
        confirm: None,
    });
    if duplicate {
        row.candidate = None;
        row.drop_ids = vec![id];
        row.after_drop = Some("next".into());
        enter(row, "drop", now);
    } else {
        row.candidate = Some(DbJson(Candidate {
            token,
            allocation_id: Some(id),
            ip: Some(ip),
        }));
        let next = if before.package_id.is_some() {
            "join"
        } else {
            "swap"
        };
        enter(row, next, now);
    }
    Ok(Flow::Again)
}

async fn join<O: Ops>(
    row: &mut Rotation,
    before: &Before,
    ops: &O,
    now: i64,
) -> Result<Flow, Failure> {
    let id = candidate_id(row)?;
    let eip = ops.eip(&id).await?.ok_or(Failure::from(CONFLICT))?;
    if eip.package_id == before.package_id {
        enter(row, "swap", now);
        return Ok(Flow::Again);
    }
    if eip.package_id.is_some() {
        return Err(CONFLICT.into());
    }
    let package = before
        .package_id
        .as_deref()
        .ok_or(Failure::from(CONFLICT))?;
    ops.join(package, &id).await?;
    Ok(Flow::Wait(2))
}

/// Moves the instance from one EIP to another, deciding only from what the cloud
/// reports now. Any address outside this pair belongs to someone else: stop.
async fn converge<O: Ops>(
    ops: &O,
    before: &Before,
    from: &str,
    to: &str,
) -> Result<Progress, Failure> {
    let instance = ops.instance().await?;
    if instance
        .attached_eip
        .as_deref()
        .is_some_and(|attached| attached != from && attached != to)
    {
        return Err(CONFLICT.into());
    }
    let here = |eip: &Eip| eip.instance_id.as_deref() == Some(before.instance_id.as_str());
    let target = ops.eip(to).await?.ok_or(Failure::from(CONFLICT))?;
    if target.status == "InUse" {
        return if here(&target) {
            Ok(Progress::Done)
        } else {
            Err(CONFLICT.into())
        };
    }
    match ops.eip(from).await? {
        Some(source) if source.status == "InUse" => {
            if !here(&source) {
                return Err(CONFLICT.into());
            }
            ops.unassociate(from).await?;
        }
        Some(source) if source.status != "Available" => {}
        _ if target.status == "Available" => ops.associate(to).await?,
        _ => {}
    }
    Ok(Progress::Pending)
}

async fn swap<O: Ops>(
    row: &mut Rotation,
    before: &Before,
    ops: &O,
    now: i64,
) -> Result<Flow, Failure> {
    let to = candidate_id(row)?;
    let from = row.bound_id.clone();
    if matches!(converge(ops, before, &from, &to).await?, Progress::Pending) {
        return Ok(Flow::Wait(1));
    }
    row.bound_id = to;
    if from == before.allocation_id {
        enter(row, "serving", now);
    } else {
        row.drop_ids = vec![from];
        row.after_drop = Some("serving".into());
        enter(row, "drop", now);
    }
    Ok(Flow::Again)
}

/// Releases an unbound address after checking it is still the one we expect.
async fn dispose<O: Ops>(ops: &O, id: &str, expected_ip: &str) -> Result<Progress, Failure> {
    let Some(eip) = ops.eip(id).await? else {
        return Ok(Progress::Done);
    };
    if eip.ip != expected_ip || eip.instance_id.is_some() || eip.status == "InUse" {
        return Err(CONFLICT.into());
    }
    if let Some(package) = &eip.package_id {
        ops.leave(package, id).await?;
    } else if eip.status == "Available" {
        ops.release(id).await?;
    }
    Ok(Progress::Pending)
}

async fn drop_next<O: Ops>(row: &mut Rotation, ops: &O, now: i64) -> Result<Flow, Failure> {
    if let Some(id) = row.drop_ids.first().cloned() {
        // Only addresses this rotation allocated may ever be released here.
        let ip = row
            .attempts
            .iter()
            .find(|a| a.allocation_id == id)
            .map(|a| a.ip.clone())
            .ok_or(Failure::from(CONFLICT))?;
        if matches!(dispose(ops, &id, &ip).await?, Progress::Pending) {
            return Ok(Flow::Wait(2));
        }
        row.drop_ids.remove(0);
        if !row.drop_ids.is_empty() {
            return Ok(Flow::Again);
        }
    }
    Ok(match row.after_drop.take().as_deref() {
        Some("serving") => {
            enter(row, "serving", now);
            Flow::Again
        }
        Some("next") => next_attempt(row, now),
        Some("finish") => {
            row.status = "restored".into();
            Flow::Wait(0)
        }
        _ => return Err(CONFLICT.into()),
    })
}

async fn serving<P: Probe>(row: &mut Rotation, probe: &P, now: i64) -> Result<Flow, Failure> {
    if probe
        .reachable(&candidate_ip(row)?, row.probe_port as u16)
        .await
    {
        enter(row, "screen", now);
        return Ok(Flow::Again);
    }
    if now - row.phase_started_at > SERVING_SECONDS {
        abort(row, "service_unreachable", now);
        return Ok(Flow::Again);
    }
    Ok(Flow::Wait(3))
}

async fn measure<P: Probe>(row: &mut Rotation, probe: &P, now: i64) -> Result<Flow, Failure> {
    let screening = row.phase == "screen";
    let plan = if screening { SCREEN } else { CONFIRM };
    let Some(measurement) = row.measurement_id.clone() else {
        let id = probe
            .start(&candidate_ip(row)?, row.probe_port as u16, plan)
            .await?;
        row.measurement_id = Some(id);
        return Ok(Flow::Wait(4));
    };
    let stalled = now - row.phase_started_at > STALL_SECONDS;
    let Some(tally) = probe.read(&measurement).await? else {
        if stalled {
            abort(row, "probe_unavailable", now);
            return Ok(Flow::Again);
        }
        return Ok(Flow::Wait(3));
    };
    row.measurement_id = None;
    record(row, |entry| {
        if screening {
            entry.screen = Some(tally);
        } else {
            entry.confirm = Some(tally);
        }
    });
    Ok(match verdict(tally, plan) {
        Verdict::Clean if screening => {
            enter(row, "confirm", now);
            Flow::Again
        }
        Verdict::Clean => {
            record(row, |entry| entry.verdict = "clean".into());
            enter(row, "release_old", now);
            Flow::Again
        }
        Verdict::Blocked => {
            record(row, |entry| entry.verdict = "blocked".into());
            next_attempt(row, now)
        }
        Verdict::Inconclusive if stalled => {
            abort(row, "probe_inconclusive", now);
            Flow::Again
        }
        Verdict::Inconclusive => Flow::Wait(10),
    })
}

async fn release_old<O: Ops>(
    row: &mut Rotation,
    before: &Before,
    ops: &O,
) -> Result<Flow, Failure> {
    let instance = ops.instance().await?;
    if instance.attached_eip.as_deref() != Some(row.bound_id.as_str()) {
        return Err(CONFLICT.into());
    }
    if matches!(
        dispose(ops, &before.allocation_id, &before.public_ip).await?,
        Progress::Pending
    ) {
        return Ok(Flow::Wait(2));
    }
    succeed(row, None);
    Ok(Flow::Wait(0))
}

async fn restore<O: Ops>(
    row: &mut Rotation,
    before: &Before,
    ops: &O,
    now: i64,
) -> Result<Flow, Failure> {
    let from = row.bound_id.clone();
    if matches!(
        converge(ops, before, &from, &before.allocation_id).await?,
        Progress::Pending
    ) {
        return Ok(Flow::Wait(1));
    }
    if let Some(candidate) = row.candidate.clone()
        && candidate.allocation_id.is_none()
    {
        // The request's outcome was never seen. Repeating its token reveals any
        // address it created, so that address can be released rather than left behind.
        let (id, ip) = ops.allocate(&request(row, before, candidate.token)).await?;
        row.attempts.push(Attempt {
            n: row.attempt,
            allocation_id: id.clone(),
            ip: ip.clone(),
            verdict: "abandoned".into(),
            screen: None,
            confirm: None,
        });
        row.candidate = Some(DbJson(Candidate {
            token: candidate.token,
            allocation_id: Some(id),
            ip: Some(ip),
        }));
    }
    let mut drops = row.drop_ids.clone();
    let unbound = row.candidate.as_ref().and_then(|c| c.allocation_id.clone());
    for id in [Some(from), unbound].into_iter().flatten() {
        if id != before.allocation_id && !drops.contains(&id) {
            drops.push(id);
        }
    }
    row.bound_id = before.allocation_id.clone();
    row.candidate = None;
    row.drop_ids = drops;
    if row.drop_ids.is_empty() {
        row.status = "restored".into();
        return Ok(Flow::Wait(0));
    }
    row.after_drop = Some("finish".into());
    enter(row, "drop", now);
    Ok(Flow::Again)
}

async fn step<O: Ops, P: Probe>(
    row: &mut Rotation,
    before: &Before,
    ops: &O,
    probe: &P,
    now: i64,
) -> Result<Flow, Failure> {
    match row.phase.as_str() {
        "allocate" => allocate(row, before, ops, now).await,
        "join" => join(row, before, ops, now).await,
        "swap" => swap(row, before, ops, now).await,
        "drop" => drop_next(row, ops, now).await,
        "serving" => serving(row, probe, now).await,
        "screen" | "confirm" => measure(row, probe, now).await,
        "release_old" => release_old(row, before, ops).await,
        "restore" => restore(row, before, ops, now).await,
        _ => Err(CONFLICT.into()),
    }
}

fn on_failure(row: &mut Rotation, before: &Before, failure: Failure, now: i64) -> Flow {
    if failure.code == CONFLICT {
        halt(row, CONFLICT);
        return Flow::Wait(0);
    }
    let rejected = matches!(failure.code, "authentication_failed" | "provider_rejected");
    let stalled = now - row.phase_started_at > STALL_SECONDS;
    match row.phase.as_str() {
        // A rejected request created nothing, so there is no token left to resolve.
        "allocate" if rejected => {
            row.candidate = None;
            if row.bound_id == before.allocation_id {
                row.status = "failed".into();
                row.error_code = Some(failure.code.into());
            } else {
                abort(row, failure.code, now);
            }
            return Flow::Wait(0);
        }
        // Includes an interrupted swap: an instance left without any address must
        // try to get its original one back before anything else.
        "allocate" | "join" | "swap" | "serving" | "screen" | "confirm" if stalled => {
            abort(row, failure.code, now);
            return Flow::Again;
        }
        // The new address already works; only the old one's release is outstanding.
        "release_old" if stalled => {
            succeed(row, Some("old_eip_retained"));
            return Flow::Wait(0);
        }
        _ if stalled => {
            halt(row, failure.code);
            return Flow::Wait(0);
        }
        _ => {}
    }
    if !restoring(row) {
        row.error_code = Some(failure.code.into());
    }
    Flow::Wait(failure.retry_after.clamp(5, MAX_RETRY_WAIT))
}

/// Stores a step over the version it was computed from. Returns false, storing
/// nothing, when the row changed in between; the next pass reads it again.
async fn save(tx: &mut Transaction<'_, Postgres>, row: &Rotation) -> ApiResult<bool> {
    let stored = sqlx::query("UPDATE alicloud_eip_rotations SET status=$2,phase=$3,phase_started_at=$4,attempt=$5,bound_id=$6,candidate=$7,measurement_id=$8,drop_ids=$9,after_drop=$10,attempts=$11,new_ip=$12,error_code=$13,updated_at=$14,next_check_at=$15,version=version+1 WHERE id=$1 AND version=$16 AND status='running'")
        .bind(row.id).bind(&row.status).bind(&row.phase).bind(row.phase_started_at).bind(row.attempt)
        .bind(&row.bound_id).bind(&row.candidate).bind(&row.measurement_id).bind(&row.drop_ids)
        .bind(&row.after_drop).bind(&row.attempts).bind(&row.new_ip).bind(&row.error_code)
        .bind(row.updated_at).bind(row.next_check_at).bind(row.version).execute(&mut **tx).await?;
    Ok(stored.rows_affected() == 1)
}

/// The resource a rotation belongs to, even if its registration was archived,
/// so a rotation can always finish and notify.
async fn owner(pool: &PgPool, id: Uuid) -> ApiResult<super::Resource> {
    Ok(
        sqlx::query_as("SELECT * FROM alicloud_resources WHERE id=$1")
            .bind(id)
            .fetch_one(pool)
            .await?,
    )
}

async fn finished(
    tx: &mut Transaction<'_, Postgres>,
    row: &Rotation,
    before: &Before,
    resource: &super::Resource,
    now: i64,
) -> ApiResult<()> {
    let reason = super::super::model::message(row.error_code.as_deref().unwrap_or_default());
    // The instance's address is known exactly when a rotation ends this way, so
    // the stored snapshot shows it at once. `now` is taken after the cloud calls:
    // a refresh pass that began earlier sees a newer `checked_at` and keeps it.
    let bound = match row.status.as_str() {
        "succeeded" => row.new_ip.as_deref().map(|ip| (ip, row.bound_id.as_str())),
        "restored" => Some((before.public_ip.as_str(), before.allocation_id.as_str())),
        _ => None,
    };
    if let Some((ip, allocation)) = bound {
        sqlx::query("UPDATE alicloud_resources SET snapshot=jsonb_set(jsonb_set(snapshot,'{public_ip}',to_jsonb($2::text)),'{attached_eip_id}',to_jsonb($3::text)),checked_at=$4 WHERE id=$1 AND snapshot IS NOT NULL")
            .bind(resource.id).bind(ip).bind(allocation).bind(now).execute(&mut **tx).await?;
    }
    let (title, message) = match row.status.as_str() {
        "succeeded" => {
            let new_ip = row.new_ip.as_deref().unwrap_or_default();
            let retained = if row.error_code.is_some() {
                format!("{reason}。")
            } else {
                String::new()
            };
            (
                "公网 IP 已更换",
                format!(
                    "{} → {new_ip}，共尝试 {} 个候选。{retained}写着旧 IP 的节点公开地址和客户端配置不会自动修改，请改为新地址；配置了域名解析的服务器会在 Agent 上报新地址后自动更新。",
                    before.public_ip, row.attempt
                ),
            )
        }
        "restored" => (
            "公网 IP 未更换，已恢复原地址",
            format!("仍为 {}。原因：{reason}", before.public_ip),
        ),
        "failed" => (
            "公网 IP 更换未开始",
            format!("仍为 {}。原因：{reason}", before.public_ip),
        ),
        _ => (
            "公网 IP 更换需要人工核对",
            format!("自动操作已停止，请到阿里云控制台核对。原因：{reason}"),
        ),
    };
    let key = format!("eip-rotation:{}:{}", row.id, row.status);
    super::super::notices::record(tx, resource, &key, title, &message, now).await
}

/// Runs one step of a rotation. Returns whether another step can run immediately.
///
/// The cloud and probe calls run with no transaction open, so the account
/// lock is never held while waiting on Alibaba Cloud or Globalping and the CDT
/// guard, power jobs and edits of the account's other resources keep moving.
/// Only this worker advances a running rotation, and the step is stored under
/// the lock only over the version it read. A step whose result was not stored
/// (a crash, or a conflicting change) is taken again: every step decides from
/// what the cloud reports now, and allocation repeats its saved client token.
pub(super) async fn advance<O: Ops, P: Probe>(
    pool: &PgPool,
    id: Uuid,
    ops: &O,
    probe: &P,
) -> ApiResult<bool> {
    let mut row = super::load(pool, id).await?;
    if row.status != "running" {
        return Ok(false);
    }
    let now = now_timestamp();
    let before = row.before.0.clone();
    let flow = match step(&mut row, &before, ops, probe, now).await {
        Ok(flow) => {
            if row.status == "running" && !restoring(&row) {
                row.error_code = None;
            }
            flow
        }
        Err(failure) => on_failure(&mut row, &before, failure, now),
    };
    let resource = owner(pool, row.resource_id).await?;
    let mut tx = super::super::lock(pool, resource.account_id).await?;
    let stored_at = now_timestamp();
    row.updated_at = stored_at;
    let again = match flow {
        Flow::Again => {
            row.next_check_at = stored_at;
            true
        }
        Flow::Wait(seconds) => {
            row.next_check_at = stored_at + seconds;
            false
        }
    };
    if !save(&mut tx, &row).await? {
        return Ok(false);
    }
    if row.status != "running" {
        finished(&mut tx, &row, &before, &resource, stored_at).await?;
    }
    tx.commit().await?;
    Ok(again && row.status == "running")
}

static LAST_PRUNE: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);

async fn tick(pool: &PgPool) -> ApiResult<()> {
    let now = now_timestamp();
    sqlx::query("UPDATE alicloud_eip_rotations SET status='cancelled',updated_at=$1 WHERE status='preview' AND expires_at<=$1")
        .bind(now).execute(pool).await?;
    if super::super::prune_due(&LAST_PRUNE, now) {
        sqlx::query("DELETE FROM alicloud_eip_rotations WHERE status IN ('cancelled','succeeded','restored','failed','reconciled') AND created_at<$1")
            .bind(now - 180 * 86400).execute(pool).await?;
    }
    let ids: Vec<Uuid> = sqlx::query_scalar("SELECT id FROM alicloud_eip_rotations WHERE status='running' AND next_check_at<=$1 ORDER BY next_check_at,created_at LIMIT 4")
        .bind(now).fetch_all(pool).await?;
    if ids.is_empty() {
        return Ok(());
    }
    let cloud = super::Cloud::new(pool).map_err(super::super::failure)?;
    let probe = super::LiveProbe::new().map_err(super::super::failure)?;
    for id in ids {
        if let Err(error) = run_steps(pool, id, &cloud, &probe).await {
            tracing::warn!(%error,%id,"Cloud EIP rotation step failed");
        }
    }
    Ok(())
}

/// Without credentials nothing can be read or changed, so a rotation that stays
/// this way must surface to a person instead of holding the resource forever.
async fn credentials_unavailable(pool: &PgPool, id: Uuid, error: Failure) -> ApiResult<()> {
    let mut row = super::load(pool, id).await?;
    if row.status != "running" {
        return Ok(());
    }
    let resource = owner(pool, row.resource_id).await?;
    let mut tx = super::super::lock(pool, resource.account_id).await?;
    let now = now_timestamp();
    row.error_code = Some(error.code.into());
    row.next_check_at = now + MAX_RETRY_WAIT;
    row.updated_at = now;
    if now - row.phase_started_at > STALL_SECONDS {
        row.status = "unknown".into();
    }
    if !save(&mut tx, &row).await? {
        return Ok(());
    }
    if row.status != "running" {
        let before = row.before.0.clone();
        finished(&mut tx, &row, &before, &resource, now).await?;
    }
    tx.commit().await?;
    Ok(())
}

pub(super) async fn run_steps<P: Probe>(
    pool: &PgPool,
    id: Uuid,
    cloud: &super::Cloud,
    probe: &P,
) -> ApiResult<()> {
    let (account, resource) = super::row_context(pool, id).await?;
    let ops = match LiveOps::new(cloud, &account, &resource).await {
        Ok(ops) => ops,
        Err(error) => return credentials_unavailable(pool, id, error).await,
    };
    // Bounded so that one rotation cannot starve the others in this pass.
    for _ in 0..8 {
        if !advance(pool, id, &ops, probe).await? {
            break;
        }
    }
    Ok(())
}

pub(in super::super) async fn run(pool: PgPool) {
    let mut timer = tokio::time::interval(std::time::Duration::from_secs(2));
    timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let (mut reported_at, mut reported_healthy) = (0, None);
    loop {
        timer.tick().await;
        let result = tick(&pool).await;
        if let Err(error) = &result {
            tracing::warn!(%error,"Cloud EIP rotation pass failed");
        }
        // The pass runs every two seconds; its health is reported every thirty
        // seconds, or at once when it changes.
        let now = now_timestamp();
        let healthy = result.is_ok();
        if now - reported_at < 30 && reported_healthy == Some(healthy) {
            continue;
        }
        (reported_at, reported_healthy) = (now, Some(healthy));
        let details = match &result {
            Ok(()) => {
                serde_json::json!({"scope":"eip_rotation_cycle","completed":true,"remote_resource_health_confirmed":false})
            }
            Err(error) => {
                serde_json::json!({"scope":"eip_rotation_cycle","completed":false,"error":error.to_string(),"remote_resource_health_confirmed":false})
            }
        };
        let status = if healthy { "healthy" } else { "failed" };
        if let Err(error) = crate::control_center::system::heartbeat(
            &pool,
            "alicloud-eip-rotation",
            status,
            details,
        )
        .await
        {
            tracing::warn!(%error,"Cloud EIP rotation heartbeat storage failed");
        }
    }
}
