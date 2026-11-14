use super::*;
use anyhow::Result;
use apsp_traffic::business::ids;
use sqlx::PgPool;

fn node(credential: &str) -> apsp_compiler::Node {
    serde_json::from_value(json!({"id":1,"name":"Example node","port":443,"public_host":"proxy.example.invalid","sni":"proxy.example.invalid","private_key":"TEST_ONLY-private-key","public_key":"TEST_ONLY-public-key","short_id":"01020304","users":[{"user_id":1,"uuid":"00000000-0000-4000-8000-000000000001","credential":credential}]})).expect("node fixture")
}

#[test]
fn client_template_rejects_unknown_fields_and_missing_outbounds() {
    let content=json!({"outbounds":[{"type":"selector","tag":"proxy","outbounds":["node-1"]},{"type":"vless","tag":"node-1","uuid":"TEST_ONLY-credential"},{"type":"direct","tag":"direct"}],"route":{"final":"proxy"}}).to_string();
    assert!(
        client::apply_definition(
            content.clone(),
            Some(&json!({"selection_groups":[],"dns":null,"route":null,"unknown_field":true}))
        )
        .is_err()
    );
    assert!(client::apply_definition(content.clone(),Some(&json!({"selection_groups":[{"tag":"proxy","outbounds":["node-2"],"default":null}],"dns":null,"route":null}))).is_err());
    assert!(
        client::apply_definition(
            content,
            Some(&json!({"selection_groups":[],"dns":null,"route":{"unsupported_rule_source":[]}}))
        )
        .is_err()
    );
}

#[test]
fn client_template_preserves_credentials_and_expressible_fields() {
    let content=json!({"outbounds":[{"type":"selector","tag":"proxy","outbounds":["node-1"]},{"type":"vless","tag":"node-1","uuid":"TEST_ONLY-credential","packet_encoding":"xudp"},{"type":"direct","tag":"direct"}],"route":{"final":"proxy"}}).to_string();
    let definition = json!({"selection_groups":[{"tag":"proxy","outbounds":["node-1"],"default":"node-1"}],"dns":{"servers":[{"type":"udp","tag":"dns-example","server":"192.0.2.53","detour":"proxy"}],"final":"dns-example"},"route":{"final":"proxy","rules":[{"domain_suffix":["example.invalid"],"action":"route","outbound":"node-1"}]}});
    let output: Value = serde_json::from_str(
        &client::apply_definition(content, Some(&definition)).expect("template"),
    )
    .expect("config");
    assert_eq!(output["dns"], definition["dns"]);
    assert_eq!(output["route"], definition["route"]);
    let original = output["outbounds"]
        .as_array()
        .expect("outbounds")
        .iter()
        .find(|v| v["tag"] == "node-1")
        .expect("retained node");
    assert_eq!(original["uuid"], "TEST_ONLY-credential");
    assert_eq!(original["packet_encoding"], "xudp");
}

#[test]
fn distinct_business_operations_cannot_be_confused_by_extra_fields() {
    assert!(
        serde_json::from_value::<Operation>(
            json!({"operation":"set_validity","user_id":1,"until":"never","reset_quota":true})
        )
        .is_err()
    );
    // The new expiry is always stated: leaving it out must not mean "no time limit".
    assert!(
        serde_json::from_value::<Operation>(json!({"operation":"set_validity","user_id":1}))
            .is_err()
    );
    assert!(
        serde_json::from_value::<Operation>(
            json!({"operation":"set_validity","user_id":1,"until":null})
        )
        .is_err()
    );
    for (until, expected) in [
        (json!({"at":1_900_000_000}), Validity::At(1_900_000_000)),
        (json!("never"), Validity::Never),
    ] {
        match serde_json::from_value::<Operation>(
            json!({"operation":"set_validity","user_id":1,"until":until}),
        ) {
            Ok(Operation::SetValidity { user_id: 1, until }) => assert_eq!(until, expected),
            _ => panic!("set_validity with {expected:?} must parse"),
        }
    }
    assert!(
        serde_json::from_value::<Operation>(
            json!({"operation":"reset_quota","user_id":1,"days":30})
        )
        .is_err()
    );
    assert!(ids(&[1, 1], false).is_err());
    assert!(ids(&[0], false).is_err());
    assert_eq!(ids(&[3, 1, 2], false).expect("fixed ids"), vec![1, 2, 3]);
}

#[test]
fn template_read_projection_hides_all_custom_dns_headers() {
    let definition = json!({"selection_groups":[{"tag":"proxy","outbounds":["node-1"],"default":"node-1"}],"dns":{"servers":[{"type":"https","tag":"private-dns","server":"dns.example.invalid","headers":{"X-Custom-Header":"TEST_ONLY-secret","Authorization":"TEST_ONLY-authorization"}}]},"route":{"final":"proxy"}});
    let public = client::redact_definition(&definition);
    assert_eq!(public["selection_groups"], definition["selection_groups"]);
    assert_eq!(public["dns"]["servers"][0]["server"], "dns.example.invalid");
    let serialized = serde_json::to_string(&public).expect("public template");
    assert!(!serialized.contains("TEST_ONLY-secret"));
    assert!(!serialized.contains("TEST_ONLY-authorization"));
}

async fn fixture(pool: &PgPool) -> Result<(i64, i64, i64, i64)> {
    let server: i64 =
        sqlx::query_scalar("INSERT INTO servers(name) VALUES('Operations fixture') RETURNING id")
            .fetch_one(pool)
            .await?;
    let user:i64=sqlx::query_scalar("INSERT INTO users(name,subscription_token) VALUES('Fixture user','TEST_ONLY-operations-token') RETURNING id").fetch_one(pool).await?;
    let node:i64=sqlx::query_scalar("INSERT INTO nodes(name,server_id,port,public_host,sni,private_key,public_key,short_id) VALUES('Fixture node',$1,443,'proxy.example.invalid','proxy.example.invalid','TEST_ONLY-private','TEST_ONLY-public','01020304') RETURNING id").bind(server).fetch_one(pool).await?;
    let package:i64=sqlx::query_scalar("INSERT INTO singbox_package_groups(name,monthly_bytes,reset_day,reset_hour,reset_minute,timezone,duration_days) VALUES('Fixture package',1000,1,0,0,'UTC',365) RETURNING id").fetch_one(pool).await?;
    let assignment:i64=sqlx::query_scalar("INSERT INTO singbox_package_assignments(user_id,request_id,package_group_id,package_name,monthly_bytes,reset_day,reset_hour,reset_minute,timezone,starts_at,expires_at) VALUES($1,$2,$3,'Fixture package',1000,1,0,0,'UTC',$4-1,$4+86400) RETURNING id").bind(user).bind(Uuid::new_v4()).bind(package).bind(apsp_protocol::now_timestamp()).fetch_one(pool).await?;
    sqlx::query("INSERT INTO singbox_user_packages(user_id,assignment_id) VALUES($1,$2)")
        .bind(user)
        .bind(assignment)
        .execute(pool)
        .await?;
    Ok((server, user, node, assignment))
}

async fn usage(
    pool: &PgPool,
    server: i64,
    user: i64,
    node: i64,
    bytes: i64,
    at: i64,
) -> Result<()> {
    let epoch = Uuid::new_v4();
    sqlx::query("INSERT INTO usage_batches(server_id,epoch,seq,payload_hash,received_at) VALUES($1,$2,1,'TEST_ONLY-hash',$3)").bind(server).bind(epoch).bind(apsp_protocol::now_timestamp()).execute(pool).await?;
    sqlx::query("INSERT INTO usage_records(server_id,epoch,seq,stat_name,user_id,node_id,uplink,downlink,period_start,period_end) VALUES($1,$2,1,$3,$4,$5,$6,0,$7-1,$7)").bind(server).bind(epoch).bind(format!("u{user}_n{node}")).bind(user).bind(node).bind(bytes).bind(at).execute(pool).await?;
    Ok(())
}

#[sqlx::test(migrations = "../panel/migrations")]
async fn quota_reset_preserves_ledger_and_late_batches_still_charge(pool: PgPool) -> Result<()> {
    let (server, user, node, _) = fixture(&pool).await?;
    let at = apsp_protocol::now_timestamp();
    usage(&pool, server, user, node, 700, at).await?;
    let mut tx = pool.begin().await?;
    snapshots::snapshot(&mut tx, &Operation::ResetQuota { user_id: user }).await?;
    let receipt = mutations::execute(
        &mut tx,
        1,
        Uuid::new_v4(),
        &Operation::ResetQuota { user_id: user },
    )
    .await?;
    assert_eq!(receipt["credit_bytes"], "700");
    tx.commit().await?;
    assert_eq!(
        sqlx::query_scalar::<_, String>(
            "SELECT used_bytes FROM singbox_entitlements($1) WHERE user_id=$2"
        )
        .bind(at)
        .bind(user)
        .fetch_one(&pool)
        .await?,
        "0"
    );
    usage(&pool, server, user, node, 100, at).await?;
    assert_eq!(
        sqlx::query_scalar::<_, String>(
            "SELECT used_bytes FROM singbox_entitlements($1) WHERE user_id=$2"
        )
        .bind(at)
        .bind(user)
        .fetch_one(&pool)
        .await?,
        "100"
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM usage_records WHERE user_id=$1")
            .bind(user)
            .fetch_one(&pool)
            .await?,
        2
    );
    Ok(())
}

#[sqlx::test(migrations = "../panel/migrations")]
async fn new_expiry_creates_snapshot_without_resetting_cycle_or_used_bytes(
    pool: PgPool,
) -> Result<()> {
    let (server, user, node, original) = fixture(&pool).await?;
    let at = apsp_protocol::now_timestamp();
    usage(&pool, server, user, node, 600, at).await?;
    let before:(i64,i64,String,i64)=sqlx::query_as("SELECT starts_at,expires_at,used_bytes,cycle_start FROM singbox_entitlements($1) WHERE user_id=$2").bind(at).bind(user).fetch_one(&pool).await?;
    let operation = Operation::SetValidity {
        user_id: user,
        until: Validity::At(before.1 + 30 * 86400),
    };
    let mut tx = pool.begin().await?;
    snapshots::snapshot(&mut tx, &operation).await?;
    let receipt = mutations::execute(&mut tx, 1, Uuid::new_v4(), &operation).await?;
    tx.commit().await?;
    assert_eq!(receipt["previous_expires_at"], before.1);
    assert_eq!(receipt["expires_at"], before.1 + 30 * 86400);
    let after:(i64,i64,String,i64)=sqlx::query_as("SELECT starts_at,expires_at,used_bytes,cycle_start FROM singbox_entitlements($1) WHERE user_id=$2").bind(at).bind(user).fetch_one(&pool).await?;
    assert_eq!(after, (before.0, before.1 + 30 * 86400, before.2, before.3));
    assert_ne!(receipt["assignment_id"], json!(original));
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM singbox_package_assignments WHERE user_id=$1"
        )
        .bind(user)
        .fetch_one(&pool)
        .await?,
        2
    );
    Ok(())
}

async fn preview_of(pool: &PgPool, operation: &Operation) -> Result<(String, Value)> {
    let mut tx = pool.begin().await?;
    let preview = snapshots::snapshot(&mut tx, operation).await?;
    tx.commit().await?;
    Ok(preview)
}

async fn used(pool: &PgPool, user: i64, at: i64) -> Result<String> {
    Ok(
        sqlx::query_scalar("SELECT used_bytes FROM singbox_entitlements($1) WHERE user_id=$2")
            .bind(at)
            .bind(user)
            .fetch_one(pool)
            .await?,
    )
}

#[sqlx::test(migrations = "../panel/migrations")]
async fn preview_fingerprint_ignores_late_usage_but_follows_group_and_assignment_changes(
    pool: PgPool,
) -> Result<()> {
    let (server, user, node, assignment) = fixture(&pool).await?;
    let group: i64 = sqlx::query_scalar(
        "INSERT INTO singbox_policy_groups(name) VALUES('Initial group') RETURNING id",
    )
    .fetch_one(&pool)
    .await?;
    let package: i64 =
        sqlx::query_scalar("SELECT package_group_id FROM singbox_package_assignments WHERE id=$1")
            .bind(assignment)
            .fetch_one(&pool)
            .await?;
    let policy = Operation::PolicyBatch {
        user_ids: vec![user],
        group_ids: vec![group],
    };
    let operations = [
        policy.clone(),
        Operation::ReplacePackage {
            user_id: user,
            package_group_id: package,
        },
        Operation::SetValidity {
            user_id: user,
            until: Validity::At(apsp_protocol::now_timestamp() + 400 * 86400),
        },
        Operation::ResetQuota { user_id: user },
    ];
    let mut previews = Vec::new();
    for operation in &operations {
        previews.push(preview_of(&pool, operation).await?.0);
    }
    // Traffic keeps arriving while an administrator reads a preview: it is no change of the
    // object, so no preview may be invalidated by it.
    usage(&pool, server, user, node, 1, apsp_protocol::now_timestamp()).await?;
    for (operation, fingerprint) in operations.iter().zip(&previews) {
        assert_eq!(&preview_of(&pool, operation).await?.0, fingerprint);
    }
    // A real change of a group still invalidates the previews that depend on it.
    sqlx::query("UPDATE singbox_policy_groups SET name='Changed group' WHERE id=$1")
        .bind(group)
        .execute(&pool)
        .await?;
    assert_ne!(preview_of(&pool, &policy).await?.0, previews[0]);
    // So does a new assignment: after an expiry change the old quota reset is stale.
    let mut tx = pool.begin().await?;
    mutations::execute(&mut tx, 1, Uuid::new_v4(), &operations[2]).await?;
    tx.commit().await?;
    assert_ne!(preview_of(&pool, &operations[3]).await?.0, previews[3]);
    Ok(())
}

#[sqlx::test(migrations = "../panel/migrations")]
async fn quota_reset_credits_the_usage_found_at_apply_not_the_previewed_figure(
    pool: PgPool,
) -> Result<()> {
    let (server, user, node, _) = fixture(&pool).await?;
    let at = apsp_protocol::now_timestamp();
    usage(&pool, server, user, node, 300, at).await?;
    let operation = Operation::ResetQuota { user_id: user };
    let (previewed, summary) = preview_of(&pool, &operation).await?;
    assert_eq!(summary["credit_bytes"], "300");
    // 450 more bytes arrive between preview and apply.
    usage(&pool, server, user, node, 450, at).await?;
    let mut tx = pool.begin().await?;
    let (current, _) = snapshots::snapshot(&mut tx, &operation).await?;
    assert_eq!(
        current, previewed,
        "late usage must not fail the apply check"
    );
    let receipt = mutations::execute(&mut tx, 1, Uuid::new_v4(), &operation).await?;
    tx.commit().await?;
    assert_eq!(receipt["credit_bytes"], "750");
    assert_eq!(used(&pool, user, at).await?, "0");
    Ok(())
}

async fn entitled(pool: &PgPool, user: i64) -> Result<(String, Option<i64>)> {
    Ok(
        sqlx::query_as("SELECT status,expires_at FROM singbox_entitlements($1) WHERE user_id=$2")
            .bind(apsp_protocol::now_timestamp())
            .bind(user)
            .fetch_one(pool)
            .await?,
    )
}

async fn apply_now(pool: &PgPool, operation: &Operation) -> Result<Value> {
    let mut tx = pool.begin().await?;
    snapshots::snapshot(&mut tx, operation).await?;
    let receipt = mutations::execute(&mut tx, 1, Uuid::new_v4(), operation).await?;
    tx.commit().await?;
    Ok(receipt)
}

async fn rejection(pool: &PgPool, user_id: i64, until: Validity) -> String {
    match preview_of(pool, &Operation::SetValidity { user_id, until }).await {
        Ok(_) => panic!("{until:?} must be rejected"),
        Err(error) => error.to_string(),
    }
}

#[sqlx::test(migrations = "../panel/migrations")]
async fn a_new_expiry_reactivates_an_expired_user_and_never_lifts_the_time_limit(
    pool: PgPool,
) -> Result<()> {
    let (_, user, _, _) = fixture(&pool).await?;
    let now = apsp_protocol::now_timestamp();
    sqlx::query(
        "UPDATE singbox_package_assignments SET starts_at=$2-172800,expires_at=$2-86400 WHERE user_id=$1",
    )
    .bind(user)
    .bind(now)
    .execute(&pool)
    .await?;
    assert_eq!(entitled(&pool, user).await?.0, "expired");
    let renew = Operation::SetValidity {
        user_id: user,
        until: Validity::At(now + 30 * 86400),
    };
    let (_, summary) = preview_of(&pool, &renew).await?;
    assert_eq!(summary["previous_expires_at"], now - 86400);
    assert_eq!(summary["new_expires_at"], now + 30 * 86400);
    assert!(
        summary["effect"].as_str().unwrap().contains("恢复可用"),
        "{summary}"
    );
    assert_eq!(
        apply_now(&pool, &renew).await?["expires_at"],
        now + 30 * 86400
    );
    assert_eq!(
        entitled(&pool, user).await?,
        ("active".into(), Some(now + 30 * 86400))
    );

    let never = Operation::SetValidity {
        user_id: user,
        until: Validity::Never,
    };
    let (_, summary) = preview_of(&pool, &never).await?;
    assert_eq!(summary["new_expires_at"], Value::Null);
    let receipt = apply_now(&pool, &never).await?;
    assert_eq!(receipt["previous_expires_at"], now + 30 * 86400);
    assert_eq!(receipt["expires_at"], Value::Null);
    assert_eq!(entitled(&pool, user).await?, ("active".into(), None));
    let stored: i64 = sqlx::query_scalar("SELECT a.expires_at FROM singbox_user_packages p JOIN singbox_package_assignments a ON a.id=p.assignment_id WHERE p.user_id=$1")
        .bind(user).fetch_one(&pool).await?;
    assert_eq!(stored, NO_EXPIRY);
    assert!(
        rejection(&pool, user, Validity::Never)
            .await
            .contains("没有变化")
    );
    // From no time limit back to a fixed expiry.
    let receipt = apply_now(
        &pool,
        &Operation::SetValidity {
            user_id: user,
            until: Validity::At(now + 7 * 86400),
        },
    )
    .await?;
    assert_eq!(receipt["previous_expires_at"], Value::Null);
    assert_eq!(entitled(&pool, user).await?.1, Some(now + 7 * 86400));
    Ok(())
}

#[sqlx::test(migrations = "../panel/migrations")]
async fn a_new_expiry_must_be_a_future_change_of_an_assigned_package(pool: PgPool) -> Result<()> {
    let (_, user, _, _) = fixture(&pool).await?;
    let now = apsp_protocol::now_timestamp();
    let current = entitled(&pool, user).await?.1.expect("fixture expiry");
    assert!(
        rejection(&pool, user, Validity::At(now - 60))
            .await
            .contains("晚于当前时间")
    );
    assert!(
        rejection(&pool, user, Validity::At(now))
            .await
            .contains("晚于当前时间")
    );
    assert!(
        rejection(&pool, user, Validity::At(current))
            .await
            .contains("没有变化")
    );
    assert!(
        rejection(&pool, user, Validity::At(NO_EXPIRY))
            .await
            .contains("超出范围")
    );
    // An earlier expiry is fine as long as it is still ahead.
    let (_, summary) = preview_of(
        &pool,
        &Operation::SetValidity {
            user_id: user,
            until: Validity::At(now + 3600),
        },
    )
    .await?;
    assert!(
        summary["effect"].as_str().unwrap().contains("提前"),
        "{summary}"
    );
    // A package that has not started cannot end before its start.
    sqlx::query(
        "UPDATE singbox_package_assignments SET starts_at=$2+86400,expires_at=$2+172800 WHERE user_id=$1",
    )
    .bind(user)
    .bind(now)
    .execute(&pool)
    .await?;
    assert!(
        rejection(&pool, user, Validity::At(now + 3600))
            .await
            .contains("开始时间")
    );
    // A user without a package has no expiry to change.
    let bare: i64 = sqlx::query_scalar(
        "INSERT INTO users(name,subscription_token) VALUES('No package','TEST_ONLY-no-package') RETURNING id",
    )
    .fetch_one(&pool)
    .await?;
    assert!(
        rejection(&pool, bare, Validity::Never)
            .await
            .contains("未分配套餐")
    );
    Ok(())
}

#[sqlx::test(migrations = "../panel/migrations")]
async fn rotation_requires_exact_new_applied_identity_and_recent_device_evidence(
    pool: PgPool,
) -> Result<()> {
    let (server, user, node_id, _) = fixture(&pool).await?;
    sqlx::query("INSERT INTO accesses(user_id,node_id,uuid,stat_name,credential) VALUES($1,$2,$3,$4,'TEST_ONLY-before')").bind(user).bind(node_id).bind(Uuid::new_v4()).bind(format!("u{user}_n{node_id}")).execute(&pool).await?;
    sqlx::query("INSERT INTO server_module_status(server_id,module,target_rev,applied_rev,healthy,updated_at) VALUES($1,'singbox',1,1,TRUE,$2)").bind(server).bind(apsp_protocol::now_timestamp()).execute(&pool).await?;
    let mut tx = pool.begin().await?;
    mutations::execute(
        &mut tx,
        1,
        Uuid::new_v4(),
        &Operation::RotateNodeCredentials {
            user_id: user,
            node_ids: vec![node_id],
        },
    )
    .await?;
    tx.commit().await?;
    let (uuid, credential): (Uuid, String) =
        sqlx::query_as("SELECT uuid,credential FROM accesses WHERE user_id=$1 AND node_id=$2")
            .bind(user)
            .bind(node_id)
            .fetch_one(&pool)
            .await?;
    let mut applied = node(&credential);
    applied.id = node_id;
    applied.users = vec![apsp_compiler::Access {
        user_id: user,
        uuid,
        credential,
    }];
    sqlx::query("INSERT INTO deployments(server_id,module,rev,bundle,bundle_sha256,source_json,created_at) VALUES($1,'singbox',2,'TEST_ONLY-bundle','TEST_ONLY-hash',$2,$3)").bind(server).bind(json!([applied])).bind(apsp_protocol::now_timestamp()).execute(&pool).await?;
    sqlx::query("UPDATE server_module_status SET target_rev=2,applied_rev=2,healthy=TRUE,updated_at=$2 WHERE server_id=$1 AND module='singbox'").bind(server).bind(apsp_protocol::now_timestamp()).execute(&pool).await?;
    sqlx::query("UPDATE runtime_publications SET dirty_at=NULL WHERE server_id=$1")
        .bind(server)
        .execute(&pool)
        .await?;
    let mut tx = pool.begin().await?;
    let offline = diagnosis::rotations(&mut tx, user).await?;
    assert_eq!(offline[0]["old_downloaded_credentials_revoked"], false);
    tx.commit().await?;
    sqlx::query("UPDATE servers SET last_seen=$2 WHERE id=$1")
        .bind(server)
        .bind(apsp_protocol::now_timestamp())
        .execute(&pool)
        .await?;
    let mut tx = pool.begin().await?;
    let online = diagnosis::rotations(&mut tx, user).await?;
    assert_eq!(online[0]["state"], "device_confirmed");
    assert_eq!(online[0]["old_downloaded_credentials_revoked"], true);
    tx.commit().await?;
    sqlx::query("UPDATE servers SET capabilities='[\"runtime:checkpoint-v1\"]' WHERE id=$1")
        .bind(server)
        .execute(&pool)
        .await?;
    let mut tx = pool.begin().await?;
    let missing_checkpoint = diagnosis::rotations(&mut tx, user).await?;
    assert_eq!(
        missing_checkpoint[0]["old_downloaded_credentials_revoked"],
        false
    );
    assert_eq!(missing_checkpoint[0]["state"], "confirmation_stale");
    tx.commit().await?;
    Ok(())
}
