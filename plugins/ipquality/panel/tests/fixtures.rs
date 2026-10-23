use super::super::*;
use crate::{AppState, auth, config::Config, diagnostics::upload_section};
use apsp_protocol::{DiagnosticSectionUpdate, now_timestamp};
use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
};
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

// Synthetic scalar fixtures: no network requests or ownership assertions.
pub(super) const IP: &str = "2001:db9::1";
pub(super) const HASH: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

pub(super) fn job(id: Uuid) -> Value {
    json!({"id":id,"plugin":"ipquality","version":PLUGIN_VERSION,"timeout_secs":300,"artifact":{"sha256":HASH,"proof":{"TEST_ONLY":"synthetic bound descriptor"}},"options":{"ip_version":"6"}})
}

pub(super) fn body(id: Uuid, at: i64) -> Value {
    json!({
        "schema":1,"plugin":"ipquality","version":PLUGIN_VERSION,"job_id":id,"ip_version":"6",
        "artifact_sha256":HASH,"source_commit":SOURCE_COMMIT,"source_sha256":SOURCE_SHA256,
        "started_at":at,"finished_at":at+1,"egress_ip":IP,
        "upstream":{"Head":{"IP":IP,"Version":"v2026-09-16"},"Type":{"Usage":{"ipapi":"hosting"}},"Factor":{"Proxy":{"ipapi":false}},"Score":{"ipapi":"0"}},
        "attempts":[
            {"seq":1,"provider":"egress-discovery","dataset":"egress","target_ip":IP,"url":"https://api64.ipify.org","status":"succeeded","attempted_at":at,"elapsed_ms":1,"http_status":200,"curl_exit":0,"response_bytes":12,"error_kind":null,"error_message":null},
            {"seq":2,"provider":"check-place-aggregator","dataset":"ipapi","target_ip":IP,"url":"https://ipinfo.check.place/fixture","status":"succeeded","attempted_at":at,"elapsed_ms":2,"http_status":200,"curl_exit":0,"response_bytes":128,"error_kind":null,"error_message":null}
        ]
    })
}

pub(super) fn chapter(
    id: Uuid,
    value: Value,
    at: i64,
    revision: u64,
    complete: bool,
) -> DiagnosticSectionUpdate {
    DiagnosticSectionUpdate {
        id,
        name: "ipquality_result".into(),
        text: value.to_string(),
        complete,
        revision,
        collected_at: at + 2,
    }
}

pub(super) fn parse_body(
    value: Value,
    at: i64,
    complete: bool,
) -> crate::error::ApiResult<Projection> {
    let id = Uuid::parse_str(value["job_id"].as_str().unwrap()).unwrap();
    let job = job(id);
    let context = SectionContext {
        server_id: 1,
        job: &job,
        created_at: at,
        expires_at: at + 600,
        job_generation: 1,
    };
    result::parse(&context, &chapter(id, value, at, 1, complete))
}

pub(super) async fn state(pool: PgPool) -> anyhow::Result<AppState> {
    AppState::new(
        pool,
        Config {
            database_url: "TEST_ONLY handled by isolated sqlx pool".into(),
            listen: "127.0.0.1:0".parse()?,
            public_url: "https://panel.example.test".into(),
            data_dir: std::env::temp_dir()
                .join(format!("apsp-ipquality-no-artifacts-{}", Uuid::new_v4())),
            admin_password: Some("TEST_ONLY initial administrator password".into()),
        },
    )
    .await
}

pub(super) async fn server(state: &AppState, token: &str) -> anyhow::Result<i64> {
    let id: i64 = sqlx::query_scalar("INSERT INTO servers(name,static_info,last_seen,capabilities) VALUES('TEST_ONLY node IP fixture',$1,$2,$3) RETURNING id")
        .bind(json!({"os":"linux","arch":"amd64","ip_addresses":["192.0.2.1"]})).bind(now_timestamp())
        .bind(json!([CAPABILITY,apsp_protocol::DIAGNOSTIC_SECTIONS_CAPABILITY,apsp_protocol::DIAGNOSTIC_SERVICE_CAPABILITY,apsp_protocol::release::ARTIFACT_SIGNATURE_CAPABILITY]))
        .fetch_one(&state.pool).await?;
    sqlx::query("INSERT INTO sessions(token_hash,server_id,expires_at) VALUES($1,$2,$3)")
        .bind(auth::hash_token(token))
        .bind(id)
        .bind(now_timestamp() + 600)
        .execute(&state.pool)
        .await?;
    Ok(id)
}

pub(super) async fn saved_job(
    state: &AppState,
    server: i64,
    id: Uuid,
    at: i64,
) -> anyhow::Result<()> {
    sqlx::query("INSERT INTO diagnostic_jobs(id,server_id,status,job,created_at,updated_at,expires_at,expected_sections,agent_completed) VALUES($1,$2,'failed',$3,$4,$4,$5,ARRAY['ipquality_result','environment'],TRUE)")
        .bind(id).bind(server).bind(job(id)).bind(at).bind(at+600).execute(&state.pool).await?;
    Ok(())
}

pub(super) async fn upload(
    state: &AppState,
    token: &str,
    update: DiagnosticSectionUpdate,
) -> crate::error::ApiResult<StatusCode> {
    let mut headers = HeaderMap::new();
    headers.insert(
        header::AUTHORIZATION,
        HeaderValue::from_str(&format!("Bearer {token}")).unwrap(),
    );
    upload_section(State(state.clone()), headers, Path(update.id), Json(update)).await
}
