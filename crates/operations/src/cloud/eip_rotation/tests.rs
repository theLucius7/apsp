use super::engine::{CONFIRM, SCREEN, Verdict, advance, run_steps, verdict};
use super::*;
use apsp_cloud_api::{
    Failure,
    test_support::{Mock, Reply},
};
use client::{Allocation, Instance, Plan};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    sync::Mutex,
};

const OLD: &str = "eip-original";
const OLD_IP: &str = "192.0.2.1";
const INSTANCE: &str = "i-testonly";
const PACKAGE: &str = "cbwp-testonly";

fn address(id: &str, ip: &str, attached: bool, package: Option<&str>) -> Eip {
    Eip {
        allocation_id: id.into(),
        ip: ip.into(),
        status: if attached { "InUse" } else { "Available" }.into(),
        instance_id: attached.then(|| INSTANCE.into()),
        package_id: package.map(Into::into),
        isp: "BGP".into(),
        netmode: "public".into(),
        internet_charge_type: "PayByTraffic".into(),
        charge_type: "PostPaid".into(),
        bandwidth: 200,
        mode: "NAT".into(),
        business_status: "Normal".into(),
        deletion_protection: false,
        security_protection: false,
        address_pool: false,
        segment: false,
        resource_group: None,
    }
}

/// An in-memory account where requests take effect immediately.
struct Fake {
    state: Mutex<FakeState>,
}
struct FakeState {
    status: String,
    eips: BTreeMap<String, Eip>,
    tokens: BTreeMap<Uuid, (String, String)>,
    ips: VecDeque<String>,
    allocations: Vec<Allocation>,
    released: Vec<String>,
    /// Peak number of addresses held at once, to show candidates do not pile up.
    peak: usize,
    fail: BTreeMap<&'static str, Failure>,
    /// Applies the allocation, then loses the response, once.
    lose_allocate_response: bool,
    /// Checks from another connection whether the account lock is free during cloud calls.
    observer: Option<(PgPool, Uuid)>,
    lock_free: Vec<bool>,
    /// Bumps the rotation's version during the next allocation, once.
    interfere: Option<(PgPool, Uuid)>,
}

/// Whether another connection could take the account's lock right now.
async fn account_lock_free(pool: &PgPool, account: Uuid) -> bool {
    let mut tx = pool.begin().await.unwrap();
    let free: bool =
        sqlx::query_scalar("SELECT pg_try_advisory_xact_lock(hashtextextended($1,739104830))")
            .bind(account.to_string())
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    tx.rollback().await.unwrap();
    free
}
impl Fake {
    fn new(ips: &[&str], package: Option<&str>) -> Self {
        let mut eips = BTreeMap::new();
        eips.insert(OLD.to_owned(), address(OLD, OLD_IP, true, package));
        Self {
            state: Mutex::new(FakeState {
                status: "Running".into(),
                eips,
                tokens: BTreeMap::new(),
                ips: ips.iter().map(|ip| (*ip).to_owned()).collect(),
                allocations: Vec::new(),
                released: Vec::new(),
                peak: 1,
                fail: BTreeMap::new(),
                lose_allocate_response: false,
                observer: None,
                lock_free: Vec::new(),
                interfere: None,
            }),
        }
    }
    fn observe(&self, pool: &PgPool, account: Uuid) {
        self.state.lock().unwrap().observer = Some((pool.clone(), account));
    }
    fn interfere(&self, pool: &PgPool, rotation: Uuid) {
        self.state.lock().unwrap().interfere = Some((pool.clone(), rotation));
    }
    /// Runs the test hooks for a cloud call, outside the state mutex.
    async fn hooks(&self) {
        let (observer, interfere) = {
            let mut state = self.state.lock().unwrap();
            (state.observer.clone(), state.interfere.take())
        };
        if let Some((pool, account)) = observer {
            let free = account_lock_free(&pool, account).await;
            self.state.lock().unwrap().lock_free.push(free);
        }
        if let Some((pool, rotation)) = interfere {
            sqlx::query("UPDATE alicloud_eip_rotations SET version=version+1 WHERE id=$1")
                .bind(rotation)
                .execute(&pool)
                .await
                .unwrap();
        }
    }
    fn fail(&self, action: &'static str, code: &'static str) {
        self.state
            .lock()
            .unwrap()
            .fail
            .insert(action, Failure::from(code));
    }
    fn heal(&self, action: &str) {
        self.state.lock().unwrap().fail.remove(action);
    }
    fn check(&self, action: &str) -> Result<(), Failure> {
        match self.state.lock().unwrap().fail.get(action) {
            Some(failure) => Err(*failure),
            None => Ok(()),
        }
    }
    fn attached(&self) -> Option<String> {
        let state = self.state.lock().unwrap();
        state
            .eips
            .values()
            .find(|eip| eip.instance_id.as_deref() == Some(INSTANCE))
            .map(|eip| eip.allocation_id.clone())
    }
    fn held(&self) -> Vec<String> {
        let state = self.state.lock().unwrap();
        state.eips.values().map(|eip| eip.ip.clone()).collect()
    }
}
impl Ops for Fake {
    async fn instance(&self) -> Result<Instance, Failure> {
        self.check("instance")?;
        let status = self.state.lock().unwrap().status.clone();
        Ok(Instance {
            status,
            attached_eip: self.attached(),
        })
    }
    async fn eip(&self, id: &str) -> Result<Option<Eip>, Failure> {
        self.check("eip")?;
        Ok(self.state.lock().unwrap().eips.get(id).cloned())
    }
    async fn allocate(&self, request: &Allocation) -> Result<(String, String), Failure> {
        self.hooks().await;
        self.check("allocate")?;
        let mut state = self.state.lock().unwrap();
        state.allocations.push(request.clone());
        if let Some(existing) = state.tokens.get(&request.token) {
            return Ok(existing.clone());
        }
        let ip = state.ips.pop_front().expect("no scripted address left");
        let id = format!("eip-candidate-{}", state.tokens.len());
        state
            .eips
            .insert(id.clone(), address(&id, &ip, false, None));
        state.tokens.insert(request.token, (id.clone(), ip.clone()));
        state.peak = state.peak.max(state.eips.len());
        if std::mem::take(&mut state.lose_allocate_response) {
            return Err("network_error".into());
        }
        Ok((id, ip))
    }
    async fn join(&self, package: &str, id: &str) -> Result<(), Failure> {
        self.check("join")?;
        let mut state = self.state.lock().unwrap();
        state.eips.get_mut(id).unwrap().package_id = Some(package.into());
        Ok(())
    }
    async fn leave(&self, _package: &str, id: &str) -> Result<(), Failure> {
        self.check("leave")?;
        let mut state = self.state.lock().unwrap();
        state.eips.get_mut(id).unwrap().package_id = None;
        Ok(())
    }
    async fn associate(&self, id: &str) -> Result<(), Failure> {
        self.hooks().await;
        self.check("associate")?;
        assert!(self.attached().is_none(), "instance already has an address");
        let mut state = self.state.lock().unwrap();
        let eip = state.eips.get_mut(id).unwrap();
        eip.status = "InUse".into();
        eip.instance_id = Some(INSTANCE.into());
        Ok(())
    }
    async fn unassociate(&self, id: &str) -> Result<(), Failure> {
        self.check("unassociate")?;
        let mut state = self.state.lock().unwrap();
        let eip = state.eips.get_mut(id).unwrap();
        eip.status = "Available".into();
        eip.instance_id = None;
        Ok(())
    }
    async fn release(&self, id: &str) -> Result<(), Failure> {
        self.hooks().await;
        self.check("release")?;
        let mut state = self.state.lock().unwrap();
        let eip = state.eips.get(id).unwrap();
        assert!(eip.instance_id.is_none(), "released a bound address");
        assert!(eip.package_id.is_none(), "released an address in a package");
        state.eips.remove(id);
        state.released.push(id.into());
        Ok(())
    }
}

struct FakeProbe {
    clean: BTreeSet<String>,
    closed: BTreeSet<String>,
    started: Mutex<Vec<(String, u16, u32)>>,
    observer: Mutex<Option<(PgPool, Uuid)>>,
    lock_free: Mutex<Vec<bool>>,
}
impl FakeProbe {
    fn new(clean: &[&str]) -> Self {
        Self {
            clean: clean.iter().map(|ip| (*ip).to_owned()).collect(),
            closed: BTreeSet::new(),
            started: Mutex::new(Vec::new()),
            observer: Mutex::new(None),
            lock_free: Mutex::new(Vec::new()),
        }
    }
    fn observe(&self, pool: &PgPool, account: Uuid) {
        *self.observer.lock().unwrap() = Some((pool.clone(), account));
    }
}
impl Probe for FakeProbe {
    async fn reachable(&self, ip: &str, _port: u16) -> bool {
        !self.closed.contains(ip)
    }
    async fn start(&self, ip: &str, port: u16, plan: Plan) -> Result<String, Failure> {
        let observer = self.observer.lock().unwrap().clone();
        if let Some((pool, account)) = observer {
            let free = account_lock_free(&pool, account).await;
            self.lock_free.lock().unwrap().push(free);
        }
        self.started
            .lock()
            .unwrap()
            .push((ip.to_owned(), port, plan.limit));
        Ok(ip.replace('.', "x"))
    }
    async fn read(&self, id: &str) -> Result<Option<Tally>, Failure> {
        let reached = if self.clean.contains(&id.replace('x', ".")) {
            10
        } else {
            1
        };
        Ok(Some(Tally { total: 10, reached }))
    }
}

async fn seed(pool: &PgPool, status: &str) -> (Account, Resource) {
    sqlx::query("INSERT INTO admins(id,password_hash) VALUES(1,'TEST_ONLY-not-a-login-password')")
        .execute(pool)
        .await
        .unwrap();
    let account = Uuid::new_v4();
    let resource = Uuid::new_v4();
    sqlx::query("INSERT INTO alicloud_accounts(id,name,access_key_id,access_key_secret) VALUES($1,'TEST_ONLY','TEST_ONLY_CLOUD_ID','TEST_ONLY_CLOUD_SECRET')").bind(account).execute(pool).await.unwrap();
    sqlx::query("INSERT INTO alicloud_resources(id,account_id,name,kind,region,cloud_id,snapshot) VALUES($1,$2,'TEST_ONLY ECS','ecs','cn-hangzhou',$3,$4)")
        .bind(resource).bind(account).bind(INSTANCE)
        .bind(json!({"kind":"ecs","cloud_id":INSTANCE,"region":"cn-hangzhou","public_ip":OLD_IP,"bandwidth_mbps":200,"charge_type":"PayByTraffic","resource_charge_type":"PostPaid","status":status,"attached_eip_id":OLD}))
        .execute(pool).await.unwrap();
    let mut tx = pool.begin().await.unwrap();
    let account = account_on(&mut tx, account).await.unwrap();
    tx.commit().await.unwrap();
    (account, super::resource(pool, resource).await.unwrap())
}

fn preview_input(attempts: i32) -> Preview {
    Preview {
        probe_port: 443,
        max_attempts: attempts,
        resource_revision: 1,
    }
}

/// Previews and confirms, leaving a rotation ready for the worker.
async fn start(pool: &PgPool, fake: &Fake, probe: &FakeProbe, attempts: i32) -> Uuid {
    let (account, resource) = seed(pool, "Running").await;
    let preview = preview_with(
        pool,
        1,
        &account,
        &resource,
        preview_input(attempts),
        fake,
        probe,
    )
    .await
    .unwrap();
    assert_eq!(preview.status, "preview");
    confirm_with(
        pool,
        1,
        preview.id,
        Confirm {
            confirm: true,
            snapshot_digest: preview.snapshot_digest.clone(),
        },
        fake,
    )
    .await
    .unwrap();
    preview.id
}

async fn drive(pool: &PgPool, id: Uuid, fake: &Fake, probe: &FakeProbe) -> Rotation {
    for _ in 0..200 {
        advance(pool, id, fake, probe).await.unwrap();
        let row = load(pool, id).await.unwrap();
        if row.status != "running" {
            return row;
        }
    }
    panic!(
        "rotation did not finish: {}",
        load(pool, id).await.unwrap().phase
    );
}
async fn age_phase(pool: &PgPool, id: Uuid) {
    sqlx::query(
        "UPDATE alicloud_eip_rotations SET phase_started_at=phase_started_at-3600 WHERE id=$1",
    )
    .bind(id)
    .execute(pool)
    .await
    .unwrap();
}
async fn events(pool: &PgPool) -> Vec<(String, String)> {
    sqlx::query_as("SELECT title,message FROM alicloud_events ORDER BY id")
        .fetch_all(pool)
        .await
        .unwrap()
}

#[sqlx::test(migrations = "../panel/migrations")]
async fn a_clean_address_replaces_the_old_one_and_the_old_one_is_released(pool: PgPool) {
    let fake = Fake::new(&["198.51.100.1"], Some(PACKAGE));
    let probe = FakeProbe::new(&["198.51.100.1"]);
    let id = start(&pool, &fake, &probe, 3).await;
    let row = drive(&pool, id, &fake, &probe).await;
    assert_eq!(row.status, "succeeded");
    assert_eq!(row.new_ip.as_deref(), Some("198.51.100.1"));
    assert_eq!(row.error_code, None);
    assert_eq!(fake.held(), ["198.51.100.1"]);
    assert_eq!(fake.attached().as_deref(), Some("eip-candidate-0"));
    {
        let state = fake.state.lock().unwrap();
        assert_eq!(state.released, [OLD]);
        assert_eq!(
            state.eips["eip-candidate-0"].package_id.as_deref(),
            Some(PACKAGE),
            "the new address keeps the old shared bandwidth package"
        );
        assert_eq!(state.allocations[0].isp, "BGP");
        assert_eq!(state.allocations[0].internet_charge_type, "PayByTraffic");
    }
    let plans: Vec<u32> = probe.started.lock().unwrap().iter().map(|s| s.2).collect();
    assert_eq!(plans, [SCREEN.limit, CONFIRM.limit]);
    let snapshot = super::resource(&pool, row.resource_id)
        .await
        .unwrap()
        .snapshot
        .unwrap();
    assert_eq!(snapshot.public_ip, "198.51.100.1");
    assert_eq!(snapshot.attached_eip_id.as_deref(), Some("eip-candidate-0"));
    let notices = events(&pool).await;
    assert_eq!(notices.len(), 1);
    assert!(notices[0].1.contains("不会自动修改"));
}

#[sqlx::test(migrations = "../panel/migrations")]
async fn blocked_candidates_are_released_as_soon_as_the_next_one_is_bound(pool: PgPool) {
    let fake = Fake::new(&["198.51.100.1", "198.51.100.2"], Some(PACKAGE));
    let probe = FakeProbe::new(&["198.51.100.2"]);
    let id = start(&pool, &fake, &probe, 3).await;
    let row = drive(&pool, id, &fake, &probe).await;
    assert_eq!(row.status, "succeeded");
    assert_eq!(row.attempt, 2);
    assert_eq!(row.attempts[0].verdict, "blocked");
    assert_eq!(row.attempts[1].verdict, "clean");
    let state = fake.state.lock().unwrap();
    assert_eq!(state.released, ["eip-candidate-0", OLD]);
    assert_eq!(state.peak, 3, "old, previous candidate and next candidate");
}

#[sqlx::test(migrations = "../panel/migrations")]
async fn when_every_candidate_is_blocked_the_original_address_returns(pool: PgPool) {
    let fake = Fake::new(&["198.51.100.1", "198.51.100.2"], Some(PACKAGE));
    let probe = FakeProbe::new(&[]);
    let id = start(&pool, &fake, &probe, 2).await;
    let row = drive(&pool, id, &fake, &probe).await;
    assert_eq!(row.status, "restored");
    assert_eq!(row.error_code.as_deref(), Some("all_candidates_blocked"));
    assert_eq!(fake.attached().as_deref(), Some(OLD));
    assert_eq!(fake.held(), [OLD_IP]);
    assert!(events(&pool).await[0].1.contains(OLD_IP));
    // The stored snapshot shows the original address again, stamped when it ended.
    let resource = super::resource(&pool, row.resource_id).await.unwrap();
    let snapshot = resource.snapshot.unwrap();
    assert_eq!(snapshot.public_ip, OLD_IP);
    assert_eq!(snapshot.attached_eip_id.as_deref(), Some(OLD));
    assert!(
        resource
            .checked_at
            .is_some_and(|checked| checked >= row.created_at)
    );
}

#[sqlx::test(migrations = "../panel/migrations")]
async fn a_new_address_that_does_not_serve_restores_instead_of_burning_candidates(pool: PgPool) {
    let fake = Fake::new(&["198.51.100.1"], None);
    let mut probe = FakeProbe::new(&["198.51.100.1"]);
    probe.closed.insert("198.51.100.1".into());
    let id = start(&pool, &fake, &probe, 5).await;
    for _ in 0..8 {
        advance(&pool, id, &fake, &probe).await.unwrap();
    }
    assert_eq!(load(&pool, id).await.unwrap().phase, "serving");
    age_phase(&pool, id).await;
    let row = drive(&pool, id, &fake, &probe).await;
    assert_eq!(row.status, "restored");
    assert_eq!(row.error_code.as_deref(), Some("service_unreachable"));
    assert_eq!(fake.attached().as_deref(), Some(OLD));
    assert_eq!(fake.held(), [OLD_IP]);
    assert_eq!(row.attempt, 1);
}

#[sqlx::test(migrations = "../panel/migrations")]
async fn an_address_seen_before_is_released_without_ever_being_bound(pool: PgPool) {
    let fake = Fake::new(&[OLD_IP, "198.51.100.1"], None);
    let probe = FakeProbe::new(&["198.51.100.1"]);
    let id = start(&pool, &fake, &probe, 2).await;
    let row = drive(&pool, id, &fake, &probe).await;
    assert_eq!(row.status, "succeeded");
    assert_eq!(row.attempts[0].verdict, "duplicate");
    assert_eq!(row.new_ip.as_deref(), Some("198.51.100.1"));
    assert_eq!(
        fake.state.lock().unwrap().released,
        ["eip-candidate-0", OLD]
    );
}

#[sqlx::test(migrations = "../panel/migrations")]
async fn a_lost_allocation_response_repeats_the_token_instead_of_buying_a_second_address(
    pool: PgPool,
) {
    let fake = Fake::new(&["198.51.100.1", "198.51.100.2"], None);
    fake.state.lock().unwrap().lose_allocate_response = true;
    let probe = FakeProbe::new(&["198.51.100.1"]);
    let id = start(&pool, &fake, &probe, 2).await;
    let row = drive(&pool, id, &fake, &probe).await;
    assert_eq!(row.status, "succeeded");
    assert_eq!(row.new_ip.as_deref(), Some("198.51.100.1"));
    let state = fake.state.lock().unwrap();
    assert_eq!(state.allocations.len(), 2);
    assert_eq!(state.allocations[0].token, state.allocations[1].token);
    assert_eq!(state.ips.len(), 1, "the second address was never allocated");
}

#[sqlx::test(migrations = "../panel/migrations")]
async fn an_allocation_that_never_answers_is_resolved_before_giving_up(pool: PgPool) {
    let fake = Fake::new(&["198.51.100.1"], None);
    let probe = FakeProbe::new(&[]);
    let id = start(&pool, &fake, &probe, 1).await;
    fake.fail("allocate", "network_error");
    advance(&pool, id, &fake, &probe).await.unwrap();
    advance(&pool, id, &fake, &probe).await.unwrap();
    age_phase(&pool, id).await;
    advance(&pool, id, &fake, &probe).await.unwrap();
    assert_eq!(load(&pool, id).await.unwrap().phase, "restore");
    fake.heal("allocate");
    let row = drive(&pool, id, &fake, &probe).await;
    assert_eq!(row.status, "restored");
    assert_eq!(row.attempts[0].verdict, "abandoned");
    assert_eq!(fake.held(), [OLD_IP]);
}

#[sqlx::test(migrations = "../panel/migrations")]
async fn a_rejected_first_allocation_fails_without_touching_the_instance(pool: PgPool) {
    let fake = Fake::new(&[], None);
    let probe = FakeProbe::new(&[]);
    let id = start(&pool, &fake, &probe, 3).await;
    fake.fail("allocate", "authentication_failed");
    let row = drive(&pool, id, &fake, &probe).await;
    assert_eq!(row.status, "failed");
    assert_eq!(row.error_code.as_deref(), Some("authentication_failed"));
    assert_eq!(fake.attached().as_deref(), Some(OLD));
    assert_eq!(fake.held(), [OLD_IP]);
}

#[sqlx::test(migrations = "../panel/migrations")]
async fn a_failing_swap_gets_the_original_address_back(pool: PgPool) {
    let fake = Fake::new(&["198.51.100.1"], None);
    let probe = FakeProbe::new(&["198.51.100.1"]);
    let id = start(&pool, &fake, &probe, 3).await;
    for _ in 0..2 {
        advance(&pool, id, &fake, &probe).await.unwrap();
    }
    assert_eq!(load(&pool, id).await.unwrap().phase, "swap");
    assert_eq!(fake.attached().as_deref(), Some(OLD));
    advance(&pool, id, &fake, &probe).await.unwrap();
    assert_eq!(fake.attached(), None, "the original address came off");
    fake.fail("associate", "provider_rejected");
    advance(&pool, id, &fake, &probe).await.unwrap();
    assert_eq!(load(&pool, id).await.unwrap().status, "running");
    age_phase(&pool, id).await;
    advance(&pool, id, &fake, &probe).await.unwrap();
    assert_eq!(load(&pool, id).await.unwrap().phase, "restore");
    fake.heal("associate");
    let row = drive(&pool, id, &fake, &probe).await;
    assert_eq!(row.status, "restored");
    assert_eq!(fake.attached().as_deref(), Some(OLD));
    assert_eq!(fake.held(), [OLD_IP]);
}

#[sqlx::test(migrations = "../panel/migrations")]
async fn someone_elses_address_stops_everything_and_releases_nothing(pool: PgPool) {
    let fake = Fake::new(&["198.51.100.1"], None);
    let probe = FakeProbe::new(&["198.51.100.1"]);
    let id = start(&pool, &fake, &probe, 3).await;
    for _ in 0..3 {
        advance(&pool, id, &fake, &probe).await.unwrap();
    }
    {
        let mut state = fake.state.lock().unwrap();
        let old = state.eips.get_mut(OLD).unwrap();
        old.status = "Available".into();
        old.instance_id = None;
        state.eips.insert(
            "eip-foreign".into(),
            address("eip-foreign", "203.0.113.9", true, None),
        );
    }
    let row = drive(&pool, id, &fake, &probe).await;
    assert_eq!(row.status, "unknown");
    assert_eq!(row.error_code.as_deref(), Some("state_changed"));
    assert!(fake.state.lock().unwrap().released.is_empty());
    assert_eq!(fake.attached().as_deref(), Some("eip-foreign"));
    let mut tx = pool.begin().await.unwrap();
    assert!(bandwidth::idle(&mut tx, row.resource_id).await.is_err());
    tx.rollback().await.unwrap();
    assert!(
        reconcile_with(
            &pool,
            1,
            id,
            Reconcile {
                process_stopped: true,
                evidence: "too short".into()
            },
            &fake
        )
        .await
        .is_err()
    );
    let closed = reconcile_with(
        &pool,
        1,
        id,
        Reconcile {
            process_stopped: true,
            evidence: "Checked the console, candidate released by hand".into(),
        },
        &fake,
    )
    .await
    .unwrap();
    assert_eq!(closed.status, "reconciled");
    assert!(fake.state.lock().unwrap().released.is_empty());
    let mut tx = pool.begin().await.unwrap();
    assert!(bandwidth::idle(&mut tx, row.resource_id).await.is_ok());
    tx.rollback().await.unwrap();
}

#[sqlx::test(migrations = "../panel/migrations")]
async fn an_old_address_that_will_not_release_still_counts_and_warns(pool: PgPool) {
    let fake = Fake::new(&["198.51.100.1"], None);
    let probe = FakeProbe::new(&["198.51.100.1"]);
    let id = start(&pool, &fake, &probe, 3).await;
    fake.fail("release", "provider_rejected");
    for _ in 0..40 {
        advance(&pool, id, &fake, &probe).await.unwrap();
        if load(&pool, id).await.unwrap().phase == "release_old" {
            advance(&pool, id, &fake, &probe).await.unwrap();
            break;
        }
    }
    age_phase(&pool, id).await;
    let row = drive(&pool, id, &fake, &probe).await;
    assert_eq!(row.status, "succeeded");
    assert_eq!(row.error_code.as_deref(), Some("old_eip_retained"));
    assert_eq!(fake.attached().as_deref(), Some("eip-candidate-0"));
    assert!(fake.held().contains(&OLD_IP.to_owned()));
    assert!(events(&pool).await[0].1.contains("手动释放"));
}

#[sqlx::test(migrations = "../panel/migrations")]
async fn a_restarted_panel_continues_from_the_saved_phase(pool: PgPool) {
    let fake = Fake::new(&["198.51.100.1"], Some(PACKAGE));
    let probe = FakeProbe::new(&["198.51.100.1"]);
    let id = start(&pool, &fake, &probe, 3).await;
    // Every step reads its state from the database, so stopping here loses nothing.
    for _ in 0..4 {
        advance(&pool, id, &fake, &probe).await.unwrap();
    }
    let phase = load(&pool, id).await.unwrap().phase;
    assert_ne!(phase, "allocate");
    let row = drive(&pool, id, &fake, &probe).await;
    assert_eq!(row.status, "succeeded");
    assert_eq!(fake.state.lock().unwrap().allocations.len(), 1);
}

#[sqlx::test(migrations = "../panel/migrations")]
async fn only_this_rotations_addresses_can_be_dropped(pool: PgPool) {
    let fake = Fake::new(&["198.51.100.1"], None);
    let probe = FakeProbe::new(&[]);
    let id = start(&pool, &fake, &probe, 3).await;
    sqlx::query(
        "UPDATE alicloud_eip_rotations SET phase='drop',drop_ids=$2,after_drop='next' WHERE id=$1",
    )
    .bind(id)
    .bind(vec![OLD])
    .execute(&pool)
    .await
    .unwrap();
    let row = drive(&pool, id, &fake, &probe).await;
    assert_eq!(row.status, "unknown");
    assert!(fake.state.lock().unwrap().released.is_empty());
    assert_eq!(fake.attached().as_deref(), Some(OLD));
}

#[sqlx::test(migrations = "../panel/migrations")]
async fn preview_refuses_anything_it_could_not_replace_faithfully(pool: PgPool) {
    let (account, resource) = seed(&pool, "Running").await;
    let probe = FakeProbe::new(&[]);
    for change in [
        |eip: &mut Eip| eip.charge_type = "PrePaid".into(),
        |eip: &mut Eip| eip.mode = "BINDED".into(),
        |eip: &mut Eip| eip.business_status = "FinancialLocked".into(),
        |eip: &mut Eip| eip.deletion_protection = true,
        |eip: &mut Eip| eip.security_protection = true,
        |eip: &mut Eip| eip.address_pool = true,
        |eip: &mut Eip| eip.segment = true,
        |eip: &mut Eip| eip.internet_charge_type = "PayByDominantTraffic".into(),
    ] {
        let fake = Fake::new(&[], None);
        change(fake.state.lock().unwrap().eips.get_mut(OLD).unwrap());
        assert!(
            preview_with(
                &pool,
                1,
                &account,
                &resource,
                preview_input(3),
                &fake,
                &probe
            )
            .await
            .is_err()
        );
    }
    // Inside a shared bandwidth package the package bills the traffic, so its
    // method is accepted; the new address is allocated by traffic and joins it.
    let fake = Fake::new(&[], Some(PACKAGE));
    fake.state
        .lock()
        .unwrap()
        .eips
        .get_mut(OLD)
        .unwrap()
        .internet_charge_type = "PayByDominantTraffic".into();
    let accepted = preview_with(
        &pool,
        1,
        &account,
        &resource,
        preview_input(3),
        &fake,
        &probe,
    )
    .await
    .unwrap();
    assert_eq!(accepted.before.internet_charge_type, "PayByDominantTraffic");
    assert_eq!(
        Allocation::charge(&accepted.before.internet_charge_type, Some(PACKAGE)),
        "PayByTraffic"
    );
    // The CDT policy grants no VPC actions; the refusal says what is missing.
    let fake = Fake::new(&[], None);
    fake.fail("eip", "authentication_failed");
    let error = preview_with(
        &pool,
        1,
        &account,
        &resource,
        preview_input(3),
        &fake,
        &probe,
    )
    .await
    .err()
    .unwrap();
    assert!(matches!(error, ApiError::Conflict(message) if message.contains("VPC 权限")));
    let fake = Fake::new(&[], None);
    fake.state.lock().unwrap().status = "Stopped".into();
    assert!(
        preview_with(
            &pool,
            1,
            &account,
            &resource,
            preview_input(3),
            &fake,
            &probe
        )
        .await
        .is_err()
    );
    let fake = Fake::new(&[], None);
    let mut closed = FakeProbe::new(&[]);
    closed.closed.insert(OLD_IP.into());
    let error = preview_with(
        &pool,
        1,
        &account,
        &resource,
        preview_input(3),
        &fake,
        &closed,
    )
    .await
    .err()
    .unwrap();
    assert!(matches!(error, ApiError::Conflict(message) if message.contains("443")));
    for invalid in [0, 13] {
        assert!(matches!(
            preview_with(
                &pool,
                1,
                &account,
                &resource,
                preview_input(invalid),
                &fake,
                &probe
            )
            .await,
            Err(ApiError::BadRequest(_))
        ));
    }
    let stale = Preview {
        resource_revision: 99,
        ..preview_input(3)
    };
    assert!(
        preview_with(&pool, 1, &account, &resource, stale, &fake, &probe)
            .await
            .is_err()
    );
    sqlx::query("INSERT INTO alicloud_resources(id,account_id,name,kind,region,cloud_id) VALUES($1,$2,'TEST_ONLY EIP','eip','cn-hangzhou',$3)")
        .bind(Uuid::new_v4()).bind(account.id).bind(OLD).execute(&pool).await.unwrap();
    assert!(
        preview_with(
            &pool,
            1,
            &account,
            &resource,
            preview_input(3),
            &fake,
            &probe
        )
        .await
        .is_err()
    );
    assert_eq!(fake.state.lock().unwrap().allocations.len(), 0);
}

#[sqlx::test(migrations = "../panel/migrations")]
async fn confirmation_is_bound_to_the_admin_the_digest_and_the_frozen_state(pool: PgPool) {
    let fake = Fake::new(&["198.51.100.1"], None);
    let probe = FakeProbe::new(&[]);
    let (account, resource) = seed(&pool, "Running").await;
    let preview = preview_with(
        &pool,
        1,
        &account,
        &resource,
        preview_input(3),
        &fake,
        &probe,
    )
    .await
    .unwrap();
    let right = || Confirm {
        confirm: true,
        snapshot_digest: preview.snapshot_digest.clone(),
    };
    assert!(matches!(
        confirm_with(&pool, 2, preview.id, right(), &fake).await,
        Err(ApiError::Forbidden(_))
    ));
    let wrong = Confirm {
        confirm: true,
        snapshot_digest: "other".into(),
    };
    assert!(
        confirm_with(&pool, 1, preview.id, wrong, &fake)
            .await
            .is_err()
    );
    let declined = Confirm {
        confirm: false,
        snapshot_digest: preview.snapshot_digest.clone(),
    };
    assert!(
        confirm_with(&pool, 1, preview.id, declined, &fake)
            .await
            .is_err()
    );
    fake.state
        .lock()
        .unwrap()
        .eips
        .get_mut(OLD)
        .unwrap()
        .bandwidth = 5;
    assert!(
        confirm_with(&pool, 1, preview.id, right(), &fake)
            .await
            .is_err()
    );
    fake.state
        .lock()
        .unwrap()
        .eips
        .get_mut(OLD)
        .unwrap()
        .bandwidth = 200;
    let running = confirm_with(&pool, 1, preview.id, right(), &fake)
        .await
        .unwrap();
    assert_eq!(running.status, "running");
    // Confirming again returns the saved intent; it never starts a second run.
    let again = confirm_with(&pool, 1, preview.id, right(), &fake)
        .await
        .unwrap();
    assert_eq!(again.status, "running");
    assert!(
        preview_with(
            &pool,
            1,
            &account,
            &resource,
            preview_input(3),
            &fake,
            &probe
        )
        .await
        .is_err(),
        "a running rotation holds the resource"
    );
}

#[sqlx::test(migrations = "../panel/migrations")]
async fn an_expired_preview_cannot_start_and_the_worker_ignores_it(pool: PgPool) {
    let fake = Fake::new(&[], None);
    let probe = FakeProbe::new(&[]);
    let (account, resource) = seed(&pool, "Running").await;
    let preview = preview_with(
        &pool,
        1,
        &account,
        &resource,
        preview_input(3),
        &fake,
        &probe,
    )
    .await
    .unwrap();
    sqlx::query("UPDATE alicloud_eip_rotations SET expires_at=1 WHERE id=$1")
        .bind(preview.id)
        .execute(&pool)
        .await
        .unwrap();
    assert!(
        confirm_with(
            &pool,
            1,
            preview.id,
            Confirm {
                confirm: true,
                snapshot_digest: preview.snapshot_digest.clone()
            },
            &fake
        )
        .await
        .is_err()
    );
    assert!(!advance(&pool, preview.id, &fake, &probe).await.unwrap());
    assert_eq!(fake.state.lock().unwrap().allocations.len(), 0);
}

#[test]
fn reachability_needs_enough_independent_probes_and_nearly_all_of_them() {
    let tally = |total, reached| Tally { total, reached };
    assert_eq!(verdict(tally(5, 5), SCREEN), Verdict::Clean);
    assert_eq!(verdict(tally(2, 2), SCREEN), Verdict::Inconclusive);
    assert_eq!(verdict(tally(5, 4), SCREEN), Verdict::Blocked);
    assert_eq!(verdict(tally(8, 7), SCREEN), Verdict::Clean);
    assert_eq!(verdict(tally(11, 11), CONFIRM), Verdict::Clean);
    assert_eq!(verdict(tally(5, 5), CONFIRM), Verdict::Inconclusive);
    assert_eq!(verdict(tally(10, 8), CONFIRM), Verdict::Blocked);
}

fn wire_instance(attached: &str) -> Reply {
    let eip = if attached.is_empty() {
        json!({"AllocationId":""})
    } else {
        json!({"AllocationId":attached})
    };
    Reply::ok(
        "DescribeInstances",
        json!({"RequestId":"read","TotalCount":1,"Instances":{"Instance":[{"InstanceId":INSTANCE,"RegionId":"cn-hangzhou","Status":"Running","EipAddress":eip}]}}),
    )
}
/// An EIP inside a shared bandwidth package is shown with the package's method,
/// as the ECS API reports production's attached EIPs; a new address cannot be
/// allocated with it.
fn wire_eip(id: &str, ip: &str, status: &str, instance: &str, package: &str) -> Reply {
    let charge = if package.is_empty() {
        "PayByTraffic"
    } else {
        "PayByDominantTraffic"
    };
    Reply::ok(
        "DescribeEipAddresses",
        json!({"RequestId":"read","TotalCount":1,"EipAddresses":{"EipAddress":[{"AllocationId":id,"IpAddress":ip,"Status":status,"InstanceId":instance,"BandwidthPackageId":package,"ISP":"BGP","Netmode":"public","InternetChargeType":charge,"ChargeType":"PostPaid","Bandwidth":"500","EipBandwidth":"200","Mode":"NAT","BusinessStatus":"Normal","DeletionProtection":false,"SecurityProtectionTypes":{"SecurityProtectionType":[]},"SegmentInstanceId":"","ResourceGroupId":"rg-testonly"}]}}),
    )
}
fn wire_gone() -> Reply {
    Reply::ok(
        "DescribeEipAddresses",
        json!({"RequestId":"read","TotalCount":0,"EipAddresses":{"EipAddress":[]}}),
    )
}
fn wire_write(action: &str) -> Reply {
    Reply::ok(action, json!({"RequestId":"write"}))
}

#[sqlx::test(migrations = "../panel/migrations")]
async fn the_cloud_receives_exact_requests_for_a_shared_bandwidth_replacement(pool: PgPool) {
    const NEW: &str = "eip-new";
    const NEW_IP: &str = "198.51.100.1";
    let mock = Mock::start(vec![
        // Preview and confirmation read the frozen state.
        wire_instance(OLD),
        wire_eip(OLD, OLD_IP, "InUse", INSTANCE, PACKAGE),
        wire_instance(OLD),
        wire_eip(OLD, OLD_IP, "InUse", INSTANCE, PACKAGE),
        // Allocation, then joining the same package.
        Reply::ok(
            "AllocateEipAddress",
            json!({"RequestId":"write","AllocationId":NEW,"EipAddress":NEW_IP}),
        ),
        wire_eip(NEW, NEW_IP, "Available", "", ""),
        wire_write("AddCommonBandwidthPackageIp"),
        wire_eip(NEW, NEW_IP, "Available", "", PACKAGE),
        // The swap: off the old address, onto the new one.
        wire_instance(OLD),
        wire_eip(NEW, NEW_IP, "Available", "", PACKAGE),
        wire_eip(OLD, OLD_IP, "InUse", INSTANCE, PACKAGE),
        wire_write("UnassociateEipAddress"),
        wire_instance(""),
        wire_eip(NEW, NEW_IP, "Available", "", PACKAGE),
        wire_eip(OLD, OLD_IP, "Available", "", PACKAGE),
        wire_write("AssociateEipAddress"),
        wire_instance(NEW),
        wire_eip(NEW, NEW_IP, "InUse", INSTANCE, PACKAGE),
        // Releasing the old address only after the new one is proven.
        wire_instance(NEW),
        wire_eip(OLD, OLD_IP, "Available", "", PACKAGE),
        wire_write("RemoveCommonBandwidthPackageIp"),
        wire_instance(NEW),
        wire_eip(OLD, OLD_IP, "Available", "", ""),
        wire_write("ReleaseEipAddress"),
        wire_instance(NEW),
        wire_gone(),
    ])
    .await;
    let cloud = Cloud::local(&mock.endpoint);
    let (account, resource) = seed(&pool, "Running").await;
    let probe = FakeProbe::new(&[NEW_IP]);
    let ops = LiveOps::new(&cloud, &account, &resource).await.unwrap();
    let preview = preview_with(
        &pool,
        1,
        &account,
        &resource,
        preview_input(3),
        &ops,
        &probe,
    )
    .await
    .unwrap();
    assert_eq!(preview.before.package_id.as_deref(), Some(PACKAGE));
    confirm_with(
        &pool,
        1,
        preview.id,
        Confirm {
            confirm: true,
            snapshot_digest: preview.snapshot_digest.clone(),
        },
        &ops,
    )
    .await
    .unwrap();
    for _ in 0..40 {
        run_steps(&pool, preview.id, &cloud, &probe).await.unwrap();
        if load(&pool, preview.id).await.unwrap().status != "running" {
            break;
        }
    }
    let row = load(&pool, preview.id).await.unwrap();
    assert_eq!(row.status, "succeeded");
    assert_eq!(row.new_ip.as_deref(), Some(NEW_IP));
    mock.exhausted();
    let requests = mock.requests();
    let allocate = &requests
        .iter()
        .find(|r| r.action == "AllocateEipAddress")
        .unwrap()
        .params;
    assert_eq!(allocate["ISP"], "BGP");
    assert_eq!(allocate["InternetChargeType"], "PayByTraffic");
    assert_eq!(allocate["InstanceChargeType"], "PostPaid");
    assert_eq!(allocate["Netmode"], "public");
    assert_eq!(allocate["Bandwidth"], "200");
    assert_eq!(allocate["ResourceGroupId"], "rg-testonly");
    assert_eq!(allocate["RegionId"], "cn-hangzhou");
    assert!(Uuid::parse_str(&allocate["ClientToken"]).is_ok());
    let join = &requests
        .iter()
        .find(|r| r.action == "AddCommonBandwidthPackageIp")
        .unwrap()
        .params;
    assert_eq!(join["BandwidthPackageId"], PACKAGE);
    assert_eq!(join["IpInstanceId"], NEW);
    let bind = &requests
        .iter()
        .find(|r| r.action == "AssociateEipAddress")
        .unwrap()
        .params;
    assert_eq!(bind["AllocationId"], NEW);
    assert_eq!(bind["InstanceId"], INSTANCE);
    assert_eq!(bind["InstanceType"], "EcsInstance");
    let release = &requests
        .iter()
        .find(|r| r.action == "ReleaseEipAddress")
        .unwrap()
        .params;
    assert_eq!(release["AllocationId"], OLD);
}

#[sqlx::test(migrations = "../panel/migrations")]
async fn lost_credentials_surface_instead_of_holding_the_resource_forever(pool: PgPool) {
    let fake = Fake::new(&["198.51.100.1"], None);
    let probe = FakeProbe::new(&[]);
    let id = start(&pool, &fake, &probe, 3).await;
    let row = load(&pool, id).await.unwrap();
    sqlx::query("UPDATE alicloud_accounts SET access_key_id='',access_key_secret='' WHERE id=$1")
        .bind(row.before.account_id)
        .execute(&pool)
        .await
        .unwrap();
    let cloud = Cloud::local("http://127.0.0.1:9/");
    run_steps(&pool, id, &cloud, &probe).await.unwrap();
    let waiting = load(&pool, id).await.unwrap();
    assert_eq!(waiting.status, "running");
    assert_eq!(waiting.error_code.as_deref(), Some("credential_invalid"));
    age_phase(&pool, id).await;
    run_steps(&pool, id, &cloud, &probe).await.unwrap();
    assert_eq!(load(&pool, id).await.unwrap().status, "unknown");
    assert_eq!(events(&pool).await.len(), 1);
}

#[sqlx::test(migrations = "../panel/migrations")]
async fn steps_never_hold_the_account_lock_while_calling_the_cloud(pool: PgPool) {
    // Cloud and probe calls can take tens of seconds; holding the account lock
    // meanwhile would stall the CDT guard, power jobs and edits of the account.
    let fake = Fake::new(&["198.51.100.1", "198.51.100.2"], Some(PACKAGE));
    let probe = FakeProbe::new(&["198.51.100.2"]);
    let id = start(&pool, &fake, &probe, 3).await;
    let account = load(&pool, id).await.unwrap().before.account_id;
    fake.observe(&pool, account);
    probe.observe(&pool, account);
    assert_eq!(drive(&pool, id, &fake, &probe).await.status, "succeeded");
    let cloud = fake.state.lock().unwrap().lock_free.clone();
    let probes = probe.lock_free.lock().unwrap().clone();
    // Two allocations, two bindings, two releases and three measurements were observed.
    assert!(
        cloud.len() >= 6 && probes.len() >= 3,
        "{cloud:?} {probes:?}"
    );
    assert!(
        cloud.iter().chain(&probes).all(|free| *free),
        "the account lock was held during a cloud call: {cloud:?} {probes:?}"
    );
}

#[sqlx::test(migrations = "../panel/migrations")]
async fn a_step_is_not_stored_over_a_change_made_meanwhile(pool: PgPool) {
    let fake = Fake::new(&["198.51.100.1"], Some(PACKAGE));
    let probe = FakeProbe::new(&["198.51.100.1"]);
    let id = start(&pool, &fake, &probe, 3).await;
    // The first step only saves the allocation's client token.
    assert!(advance(&pool, id, &fake, &probe).await.unwrap());
    let saved = load(&pool, id).await.unwrap();
    let token = saved.candidate.as_ref().unwrap().token;
    fake.interfere(&pool, id);
    assert!(!advance(&pool, id, &fake, &probe).await.unwrap());
    let kept = load(&pool, id).await.unwrap();
    assert_eq!(kept.phase, "allocate");
    assert!(kept.candidate.as_ref().unwrap().allocation_id.is_none());
    assert!(kept.attempts.is_empty());
    assert_eq!(
        kept.version,
        saved.version + 1,
        "only the other change was stored"
    );
    // Taken again, the step repeats the same token and gets the same address.
    assert_eq!(drive(&pool, id, &fake, &probe).await.status, "succeeded");
    let state = fake.state.lock().unwrap();
    assert!(state.allocations.len() >= 2);
    assert!(
        state
            .allocations
            .iter()
            .all(|request| request.token == token)
    );
    assert_eq!(state.tokens.len(), 1, "no second address was bought");
}

#[sqlx::test(migrations = "../panel/migrations")]
async fn while_it_runs_other_changes_wait_and_its_addresses_cannot_be_registered(pool: PgPool) {
    let fake = Fake::new(&["198.51.100.1"], Some(PACKAGE));
    let probe = FakeProbe::new(&["198.51.100.1"]);
    let id = start(&pool, &fake, &probe, 3).await;
    let resource = load(&pool, id).await.unwrap().resource_id;
    // Bandwidth changes, every kind of power job (manual, daily schedule,
    // keep-alive and the CDT guard) and security group changes check this first.
    let mut tx = pool.begin().await.unwrap();
    assert!(matches!(
        bandwidth::idle(&mut tx, resource).await,
        Err(ApiError::Conflict(_))
    ));
    assert!(involves(&mut tx, OLD).await.unwrap());
    tx.rollback().await.unwrap();
    while load(&pool, id).await.unwrap().attempts.is_empty() {
        advance(&pool, id, &fake, &probe).await.unwrap();
    }
    let candidate = load(&pool, id).await.unwrap().attempts[0]
        .allocation_id
        .clone();
    let mut tx = pool.begin().await.unwrap();
    assert!(involves(&mut tx, &candidate).await.unwrap());
    assert!(!involves(&mut tx, "eip-unrelated").await.unwrap());
    tx.rollback().await.unwrap();
    assert_eq!(drive(&pool, id, &fake, &probe).await.status, "succeeded");
    let mut tx = pool.begin().await.unwrap();
    bandwidth::idle(&mut tx, resource).await.unwrap();
    assert!(!involves(&mut tx, OLD).await.unwrap());
    assert!(!involves(&mut tx, &candidate).await.unwrap());
    tx.rollback().await.unwrap();
}

#[sqlx::test(migrations = "../panel/migrations")]
async fn a_preview_cannot_be_confirmed_after_the_instance_was_edited(pool: PgPool) {
    let (account, resource) = seed(&pool, "Running").await;
    let fake = Fake::new(&["198.51.100.1"], None);
    let probe = FakeProbe::new(&["198.51.100.1"]);
    let preview = preview_with(
        &pool,
        1,
        &account,
        &resource,
        preview_input(3),
        &fake,
        &probe,
    )
    .await
    .unwrap();
    // A later edit changes the revision the preview froze, so it can never be confirmed.
    sqlx::query("UPDATE alicloud_resources SET revision=revision+1 WHERE id=$1")
        .bind(resource.id)
        .execute(&pool)
        .await
        .unwrap();
    let refused = confirm_with(
        &pool,
        1,
        preview.id,
        Confirm {
            confirm: true,
            snapshot_digest: preview.snapshot_digest.clone(),
        },
        &fake,
    )
    .await;
    assert!(matches!(refused, Err(ApiError::Conflict(_))));
    assert_eq!(load(&pool, preview.id).await.unwrap().status, "preview");
    assert!(fake.state.lock().unwrap().allocations.is_empty());
}
