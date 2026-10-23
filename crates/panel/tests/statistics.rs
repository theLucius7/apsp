#![forbid(unsafe_code)]
mod business_support;
#[path = "../../protocol/tests/support/release.rs"]
mod release_support;

use anyhow::Result;
use business_support::TestPanel;
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

async fn read(panel: &TestPanel, cookie: &str, path: &str) -> Result<Value> {
    Ok(panel
        .admin(Method::GET, path, cookie, None)
        .await?
        .error_for_status()?
        .json()
        .await?)
}

#[sqlx::test]
async fn statistics_require_admin_and_preserve_missing_observations(pool: PgPool) -> Result<()> {
    let panel = TestPanel::start(pool).await?;
    sqlx::query("UPDATE panel_settings SET settings='{\"public_dashboard\":true}'")
        .execute(&panel.state.pool)
        .await?;
    let cookie = panel.admin_cookie().await?;
    for path in ["/api/statistics", "/api/plugins/sing-box/statistics"] {
        assert_eq!(
            panel
                .client
                .get(format!("{}{path}", panel.base))
                .send()
                .await?
                .status(),
            StatusCode::UNAUTHORIZED
        );
        let summary = read(&panel, &cookie, path).await?;
        assert!(summary["traffic"]["total"].is_null());
        let points = summary["points"].as_array().unwrap();
        assert_eq!(points.len(), 7);
        assert!(points.iter().all(|row| row["total"].is_null()));
        let month = read(&panel, &cookie, &format!("{path}?days=30")).await?;
        assert_eq!(month["points"].as_array().unwrap().len(), 30);
        for query in [
            "days=0",
            "days=8",
            "days=31",
            "days=255",
            "days=256",
            "days=-1",
            "user_id=1",
        ] {
            assert_eq!(
                panel
                    .admin(Method::GET, &format!("{path}?{query}"), &cookie, None)
                    .await?
                    .status(),
                StatusCode::BAD_REQUEST
            );
        }
    }
    Ok(())
}

#[sqlx::test]
async fn network_statistics_use_selected_interfaces_and_raw_observations(
    pool: PgPool,
) -> Result<()> {
    let panel = TestPanel::start(pool).await?;
    let pool = &panel.state.pool;
    let cookie = panel.admin_cookie().await?;
    let active = panel.create_server(&cookie, "selected interface").await?;
    let hidden = panel.create_server(&cookie, "hidden server").await?;
    panel.create_server(&cookie, "not enrolled").await?;
    let deleted = panel.create_server(&cookie, "deleted server").await?;
    let now = apsp_protocol::now_timestamp();
    let day = now / 86_400 * 86_400;
    sqlx::query("UPDATE servers SET device_public_key='fixture',last_seen=$2,asset_settings='{\"network_interface\":\"eth0\"}' WHERE id=$1")
        .bind(active).bind(now).execute(pool).await?;
    sqlx::query("UPDATE servers SET device_public_key='fixture-hidden',last_seen=$2,asset_settings='{\"hidden\":true}' WHERE id=$1")
        .bind(hidden).bind(now-600).execute(pool).await?;
    sqlx::query("UPDATE servers SET deleted_at=$2 WHERE id=$1")
        .bind(deleted)
        .bind(now)
        .execute(pool)
        .await?;
    for (server, at, interface, up, down, incomplete) in [
        (active, day, "eth0", "18446744073709551617", "3", false),
        (active, day, "eth1", "900", "900", false),
        (active, day - 6 * 86_400, "eth0", "2", "1", false),
        (active, day - 7 * 86_400, "eth0", "700", "0", false),
        (active, day + 86_400, "eth0", "800", "0", false),
        (hidden, day - 86_400, "eth0", "0", "0", true),
        (deleted, day, "eth0", "1000", "1000", false),
    ] {
        sqlx::query("INSERT INTO server_network_daily(server_id,day,interface,uploaded,downloaded,first_sample_at,last_sample_at,incomplete) VALUES($1,$2,$3,$4::text::numeric,$5::text::numeric,$6,$6,$7)")
            .bind(server).bind(at).bind(interface).bind(up).bind(down).bind(at*1000+1000).bind(incomplete).execute(pool).await?;
    }
    sqlx::query("INSERT INTO server_traffic_corrections(server_id,cycle_start,reset_day,network_interface,uploaded_offset,downloaded_offset,reason,created_at) VALUES($1,apsp_traffic_cycle_start($2,1),1,'eth0',999,999,'fixture',$2)")
        .bind(active).bind(now).execute(pool).await?;
    let summary = read(&panel, &cookie, "/api/statistics").await?;
    assert_eq!(
        summary["servers"],
        json!({"total":3,"online":1,"offline":1,"pending":1,"hidden":1})
    );
    assert_eq!(summary["traffic"]["uploaded"], "18446744073709551619");
    assert_eq!(summary["traffic"]["downloaded"], "4");
    assert_eq!(summary["traffic"]["total"], "18446744073709551623");
    assert_eq!(summary["traffic"]["sampled_servers"], 2);
    assert_eq!(summary["traffic"]["incomplete"], true);
    assert_eq!(summary["points"][0]["total"], "3");
    assert!(summary["points"][1]["total"].is_null());
    assert_eq!(summary["points"][5]["total"], "0");
    assert_eq!(summary["by_server"].as_array().unwrap().len(), 2);
    assert_eq!(summary["by_server"][0]["id"], active);
    assert_eq!(summary["by_server"][1]["id"], hidden);
    assert!(!summary.to_string().contains("fixture"));
    let month = read(&panel, &cookie, "/api/statistics?days=30").await?;
    assert_eq!(month["traffic"]["total"], "18446744073709552323");
    Ok(())
}

async fn usage(
    pool: &PgPool,
    server: i64,
    user: i64,
    node: i64,
    end: i64,
    amount: &str,
) -> Result<()> {
    let epoch = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO usage_batches(server_id,epoch,seq,payload_hash) VALUES($1,$2,1,'fixture')",
    )
    .bind(server)
    .bind(epoch)
    .execute(pool)
    .await?;
    sqlx::query("INSERT INTO usage_records(server_id,epoch,seq,stat_name,user_id,node_id,uplink,downlink,period_start,period_end) VALUES($1,$2,1,$3,$4,$5,$6::text::numeric,0,$7-60,$7)")
        .bind(server).bind(epoch).bind(format!("u{user}_n{node}")).bind(user).bind(node).bind(amount).bind(end).execute(pool).await?;
    Ok(())
}

#[sqlx::test]
async fn proxy_statistics_keep_history_precision_and_bounded_rankings(pool: PgPool) -> Result<()> {
    let panel = TestPanel::start(pool).await?;
    let pool = &panel.state.pool;
    let cookie = panel.admin_cookie().await?;
    let server = panel.create_server(&cookie, "proxy accounting").await?;
    let now = apsp_protocol::now_timestamp();
    let day = now / 86_400 * 86_400;
    let mut identities = Vec::new();
    for index in 0..10 {
        let user: i64 = sqlx::query_scalar(
            "INSERT INTO users(name,subscription_token) VALUES($1,$2) RETURNING id",
        )
        .bind(format!("user {index}"))
        .bind(format!("private-subscription-{index}"))
        .fetch_one(pool)
        .await?;
        let node: i64 = sqlx::query_scalar("INSERT INTO nodes(name,server_id,port,public_host,sni,private_key,public_key,short_id) VALUES($1,$2,$3,'proxy.example.com','www.example.com','private-fixture','public-fixture','01') RETURNING id")
            .bind(format!("node {index}")).bind(server).bind(20000+index).fetch_one(pool).await?;
        usage(
            pool,
            server,
            user,
            node,
            now,
            &((index + 1) * 100).to_string(),
        )
        .await?;
        identities.push((user, node));
    }
    let (user, node) = identities[9];
    sqlx::query("UPDATE users SET deleted_at=$2 WHERE id=$1")
        .bind(user)
        .bind(now)
        .execute(pool)
        .await?;
    sqlx::query("UPDATE nodes SET deleted_at=$2 WHERE id=$1")
        .bind(node)
        .bind(now)
        .execute(pool)
        .await?;
    usage(
        pool,
        server,
        user,
        node,
        day - 6 * 86_400,
        "18446744073709551615",
    )
    .await?;
    usage(pool, server, user, node, day - 6 * 86_400 - 1, "700").await?;
    usage(pool, server, user, node, now + 3600, "800").await?;
    let summary = read(&panel, &cookie, "/api/plugins/sing-box/statistics").await?;
    assert_eq!(summary["nodes"], 9);
    assert_eq!(summary["users"], 9);
    assert_eq!(summary["traffic"]["total"], "18446744073709557115");
    assert_eq!(summary["traffic"]["recorded_users"], 10);
    assert_eq!(summary["traffic"]["recorded_nodes"], 10);
    assert_eq!(summary["points"][0]["total"], "18446744073709551615");
    assert!(summary["points"][1]["total"].is_null());
    for key in ["by_user", "by_node"] {
        assert_eq!(summary[key].as_array().unwrap().len(), 8);
        assert_eq!(summary[key][0]["deleted"], true);
        assert_eq!(summary[key][0]["total"], "18446744073709552615");
    }
    assert!(!summary.to_string().contains("private"));
    assert!(!summary.to_string().contains("subscription"));
    // Retiring a server removes its nodes from current counts, not the immutable ledger.
    sqlx::query("UPDATE servers SET deleted_at=$2 WHERE id=$1")
        .bind(server)
        .bind(now)
        .execute(pool)
        .await?;
    let retired = read(&panel, &cookie, "/api/plugins/sing-box/statistics").await?;
    assert_eq!(retired["nodes"], 0);
    assert_eq!(retired["traffic"], summary["traffic"]);
    assert!(
        retired["by_node"]
            .as_array()
            .unwrap()
            .iter()
            .all(|row| row["deleted"] == true)
    );
    Ok(())
}
