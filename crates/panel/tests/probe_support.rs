#![forbid(unsafe_code)]

use apsp_protocol::{
    ProbeAddressFamily, ProbeAuthorization, ProbeAuthorizationKind, ProbeMonitor, ProbeSpec,
};
use serde_json::{Value, json};

#[allow(dead_code)]
pub fn authorize(spec: &mut ProbeSpec) {
    let identity = spec.identity();
    spec.monitor = Some(ProbeMonitor {
        network: apsp_protocol::ProbeNetwork::Other,
        region: String::new(),
        address_family: ProbeAddressFamily::Any,
        authorization: Some(ProbeAuthorization {
            kind: ProbeAuthorizationKind::Owned,
            source: "TEST_ONLY isolated fixture inventory".into(),
            scope: "Only this exact fixture target and method; never public traffic".into(),
            enabled: true,
            expires_at: None,
            identity,
        }),
    });
}

#[allow(dead_code)]
pub fn authorized(value: Value) -> Value {
    let mut spec: ProbeSpec = serde_json::from_value(value).expect("valid fixture probe shape");
    authorize(&mut spec);
    json!(spec)
}

#[allow(dead_code)]
pub async fn issued(
    pool: &sqlx::PgPool,
    client: &reqwest::Client,
    base: &str,
    server: i64,
    token: &str,
) -> anyhow::Result<apsp_protocol::ProbeLease> {
    sqlx::query("UPDATE servers SET capabilities=capabilities || $2 WHERE id=$1")
        .bind(server)
        .bind(json!([apsp_protocol::PROBE_LEASE_CAPABILITY]))
        .execute(pool)
        .await?;
    Ok(client
        .get(format!("{base}/api/agent/v1/probe-lease"))
        .bearer_auth(token)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?)
}

#[allow(dead_code)]
pub fn execution(
    lease: &apsp_protocol::ProbeLease,
    id: uuid::Uuid,
) -> anyhow::Result<apsp_protocol::ProbeExecution> {
    Ok(apsp_protocol::ProbeExecution {
        lease_id: lease.id,
        revision: lease.revision,
        issued_at: lease.issued_at,
        expires_at: lease.expires_at,
        probe: lease
            .probes
            .iter()
            .find(|probe| probe.spec.id == id)
            .ok_or_else(|| anyhow::anyhow!("fixture target was not actually issued"))?
            .clone(),
    })
}
