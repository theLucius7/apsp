#![forbid(unsafe_code)]

mod business_support;
#[path = "../../protocol/tests/support/release.rs"]
mod release_support;

use anyhow::Result;
use business_support::{TestPanel, id};
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};
use sqlx::PgPool;

fn node(server: i64, port: Value) -> Value {
    json!({"name":"Node","server_id":server,"public_host":"proxy.example.com","sni":"www.example.com","port":port})
}

async fn dirty(pool: &PgPool, server: i64) -> Result<Option<i64>> {
    Ok(
        sqlx::query_scalar("SELECT (SELECT dirty_at FROM runtime_publications WHERE server_id=$1)")
            .bind(server)
            .fetch_one(pool)
            .await?,
    )
}

async fn clean(pool: &PgPool, server: i64) -> Result<()> {
    sqlx::query("UPDATE runtime_publications SET dirty_at=NULL WHERE server_id=$1")
        .bind(server)
        .execute(pool)
        .await?;
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn explicit_ports_are_validated_and_updates_only_dirty_actual_changes(
    pool: PgPool,
) -> Result<()> {
    let panel = TestPanel::start(pool.clone()).await?;
    let cookie = panel.admin_cookie().await?;
    let server = panel.create_server(&cookie, "Ports").await?;
    panel.enable_runtime(&cookie, server).await?;
    let create = node(server, json!(443));
    assert_eq!(
        panel
            .client
            .post(format!("{}/api/traffic/nodes", panel.base))
            .json(&create)
            .send()
            .await?
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let created: Value = panel
        .admin(Method::POST, "/api/traffic/nodes", &cookie, Some(create))
        .await?
        .error_for_status()?
        .json()
        .await?;
    let id = id(&created)?;
    let path = format!("/api/traffic/nodes/{id}");
    assert_eq!(created["port"], 443);
    assert!(dirty(&pool, server).await?.is_some());
    let other = panel.create_node(&cookie, server, "Default").await?;
    assert_eq!(other["port"], 20000);
    clean(&pool, server).await?;
    assert_eq!(
        panel
            .client
            .patch(format!("{}{path}", panel.base))
            .json(&json!({"port":444}))
            .send()
            .await?
            .status(),
        StatusCode::UNAUTHORIZED
    );
    for port in [json!(0), json!(-1), json!(65536), json!(18085)] {
        for (method, endpoint, request) in [
            (
                Method::POST,
                "/api/traffic/nodes",
                node(server, port.clone()),
            ),
            (Method::PATCH, path.as_str(), json!({"port":port})),
        ] {
            let response = panel
                .admin(method, endpoint, &cookie, Some(request))
                .await?;
            assert_eq!(response.status(), StatusCode::BAD_REQUEST);
            assert!(
                response.json::<Value>().await?["error"]
                    .as_str()
                    .unwrap()
                    .contains("端口")
            );
        }
    }
    for port in [json!(1.5), json!("443"), json!(true)] {
        assert!(
            panel
                .admin(
                    Method::POST,
                    "/api/traffic/nodes",
                    &cookie,
                    Some(node(server, port.clone()))
                )
                .await?
                .status()
                .is_client_error()
        );
        assert!(
            panel
                .admin(Method::PATCH, &path, &cookie, Some(json!({"port":port})))
                .await?
                .status()
                .is_client_error()
        );
    }
    for request in [
        json!({"port":443}),
        json!({"name":"Node"}),
        json!({"public_host":"proxy.example.com","sni":"www.example.com"}),
    ] {
        let updated: Value = panel
            .admin(Method::PATCH, &path, &cookie, Some(request))
            .await?
            .error_for_status()?
            .json()
            .await?;
        assert_eq!(updated["port"], 443);
        assert_eq!(dirty(&pool, server).await?, None);
    }
    let conflict = panel
        .admin(Method::PATCH, &path, &cookie, Some(json!({"port":20000})))
        .await?;
    assert_eq!(conflict.status(), StatusCode::CONFLICT);
    assert!(
        conflict.json::<Value>().await?["error"]
            .as_str()
            .unwrap()
            .contains("已被其他节点使用")
    );
    assert_eq!(dirty(&pool, server).await?, None);
    for port in [1, 65535, 444] {
        let updated: Value = panel
            .admin(Method::PATCH, &path, &cookie, Some(json!({"port":port})))
            .await?
            .error_for_status()?
            .json()
            .await?;
        assert_eq!(updated["port"], port);
        assert!(dirty(&pool, server).await?.is_some());
        clean(&pool, server).await?;
    }
    let renamed: Value = panel
        .admin(
            Method::PATCH,
            &path,
            &cookie,
            Some(json!({"name":"Renamed"})),
        )
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(renamed["port"], 444);
    assert!(dirty(&pool, server).await?.is_some());
    assert_eq!(
        panel
            .admin(
                Method::PATCH,
                "/api/traffic/nodes/999999",
                &cookie,
                Some(json!({"port":443}))
            )
            .await?
            .status(),
        StatusCode::NOT_FOUND
    );
    panel
        .admin(Method::DELETE, &path, &cookie, None)
        .await?
        .error_for_status()?;
    assert_eq!(
        panel
            .admin(Method::PATCH, &path, &cookie, Some(json!({"port":443})))
            .await?
            .status(),
        StatusCode::NOT_FOUND
    );
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn concurrent_port_reservations_are_atomic_and_released_after_deletion(
    pool: PgPool,
) -> Result<()> {
    let panel = TestPanel::start(pool.clone()).await?;
    let cookie = panel.admin_cookie().await?;
    let server = panel.create_server(&cookie, "Atomic ports").await?;
    panel.enable_runtime(&cookie, server).await?;
    let (a, b) = tokio::join!(
        panel.admin(
            Method::POST,
            "/api/traffic/nodes",
            &cookie,
            Some(node(server, json!(20000)))
        ),
        panel.admin(
            Method::POST,
            "/api/traffic/nodes",
            &cookie,
            Some(node(server, json!(20000)))
        )
    );
    let mut statuses = vec![];
    for response in [a?, b?] {
        statuses.push(response.status().as_u16());
    }
    statuses.sort();
    assert_eq!(statuses, vec![201, 409]);
    let automatic = panel.create_node(&cookie, server, "Automatic").await?;
    assert_eq!(automatic["port"], 20001);
    let next = panel.create_node(&cookie, server, "Next").await?;
    assert_eq!(next["port"], 20002);
    let first_path = format!("/api/traffic/nodes/{}", id(&automatic)?);
    let second_path = format!("/api/traffic/nodes/{}", id(&next)?);
    let (a, b) = tokio::join!(
        panel.admin(
            Method::PATCH,
            &first_path,
            &cookie,
            Some(json!({"port":443}))
        ),
        panel.admin(
            Method::PATCH,
            &second_path,
            &cookie,
            Some(json!({"port":443}))
        )
    );
    let mut statuses = vec![];
    let mut winner = 0;
    for response in [a?, b?] {
        statuses.push(response.status().as_u16());
        if response.status() == StatusCode::OK {
            winner = id(&response.json::<Value>().await?)?;
        }
    }
    statuses.sort();
    assert_eq!(statuses, vec![200, 409]);
    let another = panel.create_server(&cookie, "Other server").await?;
    panel.enable_runtime(&cookie, another).await?;
    assert_eq!(
        panel
            .admin(
                Method::POST,
                "/api/traffic/nodes",
                &cookie,
                Some(node(another, json!(443)))
            )
            .await?
            .status(),
        StatusCode::CREATED
    );
    panel
        .admin(
            Method::DELETE,
            &format!("/api/traffic/nodes/{winner}"),
            &cookie,
            None,
        )
        .await?
        .error_for_status()?;
    assert_eq!(
        panel
            .admin(
                Method::POST,
                "/api/traffic/nodes",
                &cookie,
                Some(node(server, json!(443)))
            )
            .await?
            .status(),
        StatusCode::CREATED
    );
    Ok(())
}
