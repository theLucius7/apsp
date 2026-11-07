#![forbid(unsafe_code)]

mod business_support;
#[path = "../../protocol/tests/support/release.rs"]
mod release_support;
use anyhow::Result;
use business_support::TestPanel;
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};
use sqlx::PgPool;

#[sqlx::test(migrations = "./migrations")]
#[ignore = "Requires PostgreSQL; set DATABASE_URL and run --ignored"]
async fn limits_persist_reject_stale_writes_and_mark_hosts(pool: PgPool) -> Result<()> {
    let fixture = TestPanel::start(pool.clone()).await?;
    let cookie = fixture.admin_cookie().await?;
    let user = fixture.create_user(&cookie, "限额测试").await?;
    let id = user["id"].as_i64().unwrap();
    let path = format!("/api/plugins/sing-box/users/{id}/limits");
    let initial: Value = fixture
        .admin(Method::GET, &path, &cookie, None)
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(initial["revision"], 0);
    assert_eq!(initial["max_connections"], 0);
    let server = fixture.create_server(&cookie, "限制主机").await?;
    let node = fixture.create_node(&cookie, server, "限制节点").await?;
    let node = node["id"].as_i64().unwrap();
    sqlx::query("INSERT INTO accesses(user_id,node_id,uuid,stat_name) VALUES($1,$2,$3,$4)")
        .bind(id)
        .bind(node)
        .bind(uuid::Uuid::new_v4())
        .bind(apsp_compiler::stat_name(id, node))
        .execute(&pool)
        .await?;
    sqlx::query("UPDATE servers SET dirty_at=NULL WHERE id=$1")
        .bind(server)
        .execute(&pool)
        .await?;
    let body =
        json!({"expected_revision":0,"upload_mbps":8,"download_mbps":16,"max_connections":2});
    let saved: Value = fixture
        .admin(Method::PUT, &path, &cookie, Some(body.clone()))
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(saved["revision"], 1);
    let dirty: bool = sqlx::query_scalar("SELECT dirty_at IS NOT NULL FROM servers WHERE id=$1")
        .bind(server)
        .fetch_one(&pool)
        .await?;
    assert!(dirty);
    let stale = fixture
        .admin(Method::PUT, &path, &cookie, Some(body))
        .await?;
    assert_eq!(stale.status(), StatusCode::CONFLICT);
    let invalid = fixture.admin(Method::PUT,&path,&cookie,Some(json!({"expected_revision":1,"upload_mbps":1000001,"download_mbps":0,"max_connections":0}))).await?;
    assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);
    let value: Value = fixture
        .admin(Method::GET, &path, &cookie, None)
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(value["servers"][0]["state"], "pending");
    assert_eq!(value["download_mbps"], 16);
    assert_eq!(
        fixture
            .client
            .get(format!("{}{path}", fixture.base))
            .send()
            .await?
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let reset:Value=fixture.admin(Method::PUT,&path,&cookie,Some(json!({"expected_revision":1,"upload_mbps":0,"download_mbps":0,"max_connections":0}))).await?.error_for_status()?.json().await?;
    assert_eq!(reset["revision"], 2);
    assert_eq!(reset["max_connections"], 0);
    Ok(())
}
