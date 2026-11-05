#![forbid(unsafe_code)]
//! External nodes reach proxy users only through policy groups (ADR 0112): a
//! group binds each node to its current version or to one pinned version, and a
//! user's subscription resolves the union of their groups' bindings.
mod business_support;
#[path = "../../protocol/tests/support/release.rs"]
mod release_support;

use anyhow::{Context, Result, ensure};
use apsp_panel::plugins::singbox::sources::parse::{PARSER_VERSION, digest};
use business_support::{TestPanel, id};
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};
use sqlx::PgPool;

const ROOT: &str = "/api/plugins/sing-box";
const FIRST: &str = "TEST_ONLY_external_provider_first";
const SECOND: &str = "TEST_ONLY_external_provider_second";
const THIRD: &str = "TEST_ONLY_external_provider_third";
/// A parser older than the one that produces numbered-source versions now.
const OUTDATED_PARSER: &str = "apsp-subscriptions-1";

async fn source(pool: &PgPool) -> Result<(i64, i64)> {
    let source:i64=sqlx::query_scalar("INSERT INTO singbox_subscription_sources(name,kind,secret_content,created_at) VALUES('来源','inline','TEST_ONLY content',0) RETURNING id").fetch_one(pool).await?;
    let node:i64=sqlx::query_scalar("INSERT INTO singbox_external_nodes(source_id,identity_epoch,identity_key,name,adopted) VALUES($1,1,'stable','外部',TRUE) RETURNING id").bind(source).fetch_one(pool).await?;
    version(pool, source, node, FIRST).await?;
    Ok((source, node))
}

async fn version(pool: &PgPool, source: i64, node: i64, password: &str) -> Result<i64> {
    parsed_version(pool, source, node, password, PARSER_VERSION).await
}

async fn parsed_version(
    pool: &PgPool,
    source: i64,
    node: i64,
    password: &str,
    parser: &str,
) -> Result<i64> {
    let config = json!({"type":"shadowsocks","server":"provider.example.com","server_port":443,"method":"aes-128-gcm","password":password});
    let hash = digest(&serde_json::to_vec(&config)?);
    let revision:i64=sqlx::query_scalar("INSERT INTO singbox_source_revisions(source_id,settings_revision,identity_epoch,parser_version,body_sha256,format,supported_count,unsupported_count,fetched_at) VALUES($1,1,1,$2,$3,'singbox_json',1,0,1) RETURNING id").bind(source).bind(parser).bind(&hash).fetch_one(pool).await?;
    let version:i64=sqlx::query_scalar("INSERT INTO singbox_external_node_versions(external_node_id,source_id,source_revision_id,identity_epoch,parser_version,name,config_json,config_sha256,capabilities_json,created_at) VALUES($1,$2,$3,1,$4,'外部',$5,$6,'{}',1) RETURNING id").bind(node).bind(source).bind(revision).bind(parser).bind(config).bind(hash).fetch_one(pool).await?;
    sqlx::query("UPDATE singbox_external_nodes SET current_version_id=$2,last_seen_revision_id=$3 WHERE id=$1").bind(node).bind(version).bind(revision).execute(pool).await?;
    sqlx::query("UPDATE singbox_subscription_sources SET current_revision_id=$2,last_success_at=1 WHERE id=$1").bind(source).bind(revision).execute(pool).await?;
    Ok(version)
}

/// Administrator listings describe external nodes without their configuration.
fn redacted(text: &str) -> Result<()> {
    ensure!(
        [FIRST, SECOND, THIRD, "config_json", "secret_content"]
            .iter()
            .all(|secret| !text.contains(secret)),
        "listing exposes provider configuration: {text}"
    );
    Ok(())
}

/// Adopted nodes a group can take now, each bound to its current version.
async fn candidates(panel: &TestPanel, cookie: &str) -> Result<Vec<Value>> {
    let response = panel
        .admin(
            Method::GET,
            &format!("{ROOT}/external-node-candidates"),
            cookie,
            None,
        )
        .await?
        .error_for_status()?;
    ensure!(response.headers()["cache-control"] == "no-store");
    let text = response.text().await?;
    redacted(&text)?;
    let value: Value = serde_json::from_str(&text)?;
    Ok(value["available_nodes"]
        .as_array()
        .context("available nodes")?
        .clone())
}

async fn first_candidate(panel: &TestPanel, cookie: &str) -> Result<Value> {
    candidates(panel, cookie)
        .await?
        .into_iter()
        .next()
        .context("no external node candidate")
}

async fn groups(panel: &TestPanel, cookie: &str) -> Result<Vec<Value>> {
    let text = panel
        .admin(Method::GET, &format!("{ROOT}/policy-groups"), cookie, None)
        .await?
        .error_for_status()?
        .text()
        .await?;
    redacted(&text)?;
    Ok(serde_json::from_str(&text)?)
}

fn reference(entry: &Value, mode: &str) -> Value {
    let mut result = json!({"update_mode":mode});
    for key in [
        "external_node_id",
        "source_id",
        "identity_epoch",
        "node_version_id",
        "metadata_revision",
    ] {
        result[key] = entry[key].clone();
    }
    result
}

/// A policy group that holds external nodes only.
struct Group {
    id: i64,
    name: String,
}

impl Group {
    async fn create(panel: &TestPanel, cookie: &str, name: &str) -> Result<Self> {
        let response = panel
            .admin(
                Method::POST,
                &format!("{ROOT}/policy-groups"),
                cookie,
                Some(json!({"name":name,"node_ids":[],"chain_ids":[],"external_nodes":[]})),
            )
            .await?;
        ensure!(
            response.status() == StatusCode::CREATED,
            "policy group creation failed: {}",
            response.text().await?
        );
        let text = response.text().await?;
        redacted(&text)?;
        let value: Value = serde_json::from_str(&text)?;
        ensure!(value["external_nodes"] == json!([]), "{value}");
        Ok(Self {
            id: id(&value)?,
            name: name.into(),
        })
    }

    /// The group that carries one user's external nodes in these tests.
    async fn for_user(panel: &TestPanel, cookie: &str, user: i64) -> Result<Self> {
        let name = format!("external-{user}");
        match groups(panel, cookie)
            .await?
            .iter()
            .find(|group| group["name"] == name.as_str())
        {
            Some(group) => Ok(Self {
                id: id(group)?,
                name,
            }),
            None => Self::create(panel, cookie, &name).await,
        }
    }

    /// Replaces the group's external nodes, keeping its name and members.
    async fn put(
        &self,
        panel: &TestPanel,
        cookie: &str,
        external_nodes: Value,
    ) -> Result<(StatusCode, Value)> {
        save_group(panel, cookie, self.id, &self.name, external_nodes).await
    }

    async fn save(&self, panel: &TestPanel, cookie: &str, external_nodes: Value) -> Result<Value> {
        let (status, value) = self.put(panel, cookie, external_nodes).await?;
        ensure!(
            status == StatusCode::OK,
            "group save failed: {status}: {value}"
        );
        Ok(value)
    }

    /// The group as the policy-group listing shows it.
    async fn listed(&self, panel: &TestPanel, cookie: &str) -> Result<Value> {
        groups(panel, cookie)
            .await?
            .into_iter()
            .find(|group| group["id"] == self.id)
            .context("policy group listing")
    }

    /// The external nodes the group holds, resolved as a subscription would.
    async fn held(&self, panel: &TestPanel, cookie: &str) -> Result<Vec<Value>> {
        Ok(self.listed(panel, cookie).await?["external_nodes"]
            .as_array()
            .context("group external nodes")?
            .clone())
    }
}

async fn save_group(
    panel: &TestPanel,
    cookie: &str,
    group: i64,
    name: &str,
    external_nodes: Value,
) -> Result<(StatusCode, Value)> {
    let response = panel
        .admin(
            Method::PUT,
            &format!("{ROOT}/policy-groups/{group}"),
            cookie,
            Some(json!({"name":name,"node_ids":[],"chain_ids":[],"external_nodes":external_nodes})),
        )
        .await?;
    let status = response.status();
    let text = response.text().await?;
    redacted(&text)?;
    let value = serde_json::from_str(&text).unwrap_or(Value::String(text));
    Ok((status, value))
}

async fn user_groups(panel: &TestPanel, cookie: &str, user: i64) -> Result<Vec<i64>> {
    let value: Value = panel
        .admin(
            Method::GET,
            &format!("{ROOT}/users/{user}/policy-groups"),
            cookie,
            None,
        )
        .await?
        .error_for_status()?
        .json()
        .await?;
    Ok(value["group_ids"]
        .as_array()
        .context("group ids")?
        .iter()
        .filter_map(Value::as_i64)
        .collect())
}

async fn set_groups(panel: &TestPanel, cookie: &str, user: i64, groups: &[i64]) -> Result<()> {
    panel
        .admin(
            Method::PUT,
            &format!("{ROOT}/users/{user}/policy-groups"),
            cookie,
            Some(json!({"group_ids":groups})),
        )
        .await?
        .error_for_status()?;
    Ok(())
}

/// Adds a group to the user's groups, keeping the others.
async fn join(panel: &TestPanel, cookie: &str, user: i64, group: i64) -> Result<()> {
    let mut current = user_groups(panel, cookie, user).await?;
    if !current.contains(&group) {
        current.push(group);
    }
    set_groups(panel, cookie, user, &current).await
}

/// Puts the first candidate into the user's group with `mode` and assigns the
/// group to the user. Returns the group and the saved group.
async fn assign(panel: &TestPanel, cookie: &str, user: i64, mode: &str) -> Result<(Group, Value)> {
    let group = Group::for_user(panel, cookie, user).await?;
    let binding = reference(&first_candidate(panel, cookie).await?, mode);
    let saved = group.save(panel, cookie, json!([binding])).await?;
    join(panel, cookie, user, group.id).await?;
    Ok((group, saved))
}

async fn subscription(panel: &TestPanel, user: &Value) -> Result<reqwest::Response> {
    Ok(panel
        .client
        .get(format!(
            "{}?format=singbox",
            user["subscription_url"].as_str().context("url")?
        ))
        .send()
        .await?)
}

async fn content(panel: &TestPanel, user: &Value) -> Result<String> {
    Ok(subscription(panel, user)
        .await?
        .error_for_status()?
        .text()
        .await?)
}

async fn status(panel: &TestPanel, user: &Value) -> Result<StatusCode> {
    Ok(subscription(panel, user).await?.status())
}

async fn preview(panel: &TestPanel, cookie: &str, user: i64) -> Result<Value> {
    Ok(panel
        .admin(
            Method::GET,
            &format!("{ROOT}/users/{user}/subscription"),
            cookie,
            None,
        )
        .await?
        .error_for_status()?
        .json()
        .await?)
}

/// The external bindings the user's diagnosis reports: one per node.
async fn authorizations(panel: &TestPanel, cookie: &str, user: i64) -> Result<Vec<Value>> {
    let value: Value = panel
        .admin(
            Method::GET,
            &format!("{ROOT}/users/{user}/diagnosis"),
            cookie,
            None,
        )
        .await?
        .error_for_status()?
        .json()
        .await?;
    Ok(value["external_authorizations"]
        .as_array()
        .context("external authorizations")?
        .clone())
}

/// A refusal that names the policy groups still holding the node.
async fn held_by_group(response: reqwest::Response) -> Result<()> {
    let status = response.status();
    let value: Value = response.json().await?;
    ensure!(
        status == StatusCode::CONFLICT
            && value["error"]
                .as_str()
                .is_some_and(|message| message.contains("策略组")),
        "{status}: {value}"
    );
    Ok(())
}

#[sqlx::test]
async fn group_external_nodes_are_atomic_redacted_validated_and_revocable(
    pool: PgPool,
) -> Result<()> {
    let (source, node) = source(&pool).await?;
    let panel = TestPanel::start(pool.clone()).await?;
    let cookie = panel.admin_cookie().await?;
    let user = panel.create_user(&cookie, "外部用户").await?;
    let uid = id(&user)?;
    for path in ["external-node-candidates", "policy-groups"] {
        ensure!(
            panel
                .client
                .get(format!("{}{ROOT}/{path}", panel.base))
                .send()
                .await?
                .status()
                == StatusCode::UNAUTHORIZED,
            "{path} needs an administrator"
        );
    }
    let group = Group::for_user(&panel, &cookie, uid).await?;
    ensure!(group.held(&panel, &cookie).await?.is_empty());
    let (_, assigned) = assign(&panel, &cookie, uid, "follow_node").await?;
    let entry = &assigned["external_nodes"][0];
    ensure!(
        assigned["external_nodes"].as_array().map(Vec::len) == Some(1)
            && entry["external_node_id"] == node
            && entry["update_mode"] == "follow_node"
            && entry["available"] == true,
        "{assigned}"
    );
    let version = entry["current_version_id"].clone();

    // While a group holds the node, it cannot be unadopted or deleted, and its
    // source cannot be deleted.
    held_by_group(
        panel
            .admin(
                Method::PATCH,
                &format!("{ROOT}/subscription-sources/{source}/nodes/{node}"),
                &cookie,
                Some(json!({"adopted":false,"settings_revision":1,"identity_epoch":1,"node_version_id":version,"metadata_revision":0})),
            )
            .await?,
    )
    .await?;
    let catalog: Vec<Value> = panel
        .admin(Method::GET, &format!("{ROOT}/node-catalog"), &cookie, None)
        .await?
        .error_for_status()?
        .json()
        .await?;
    let listed = catalog
        .iter()
        .find(|item| item["kind"] == "external" && item["id"] == node)
        .context("catalog entry")?;
    held_by_group(
        panel
            .admin(
                Method::DELETE,
                &format!("{ROOT}/node-catalog/batch"),
                &cookie,
                Some(
                    json!({"items":[{"kind":"external","id":node,"revision":listed["revision"]}]}),
                ),
            )
            .await?,
    )
    .await?;
    held_by_group(
        panel
            .admin(
                Method::DELETE,
                &format!("{ROOT}/subscription-sources/{source}"),
                &cookie,
                None,
            )
            .await?,
    )
    .await?;
    ensure!(content(&panel, &user).await?.contains(FIRST));
    ensure!(
        panel
            .client
            .get(user["subscription_url"].as_str().context("url")?)
            .send()
            .await?
            .status()
            == StatusCode::CONFLICT
    );

    let saved = reference(entry, "follow_node");
    let (rejected, _) = group
        .put(&panel, &cookie, json!([saved.clone(), saved.clone()]))
        .await?;
    ensure!(
        rejected == StatusCode::BAD_REQUEST,
        "duplicate node: {rejected}"
    );
    // A refused replacement leaves the whole group as it was, its name included.
    let mut forged = saved.clone();
    forged["source_id"] = json!(999999);
    let (rejected, _) = save_group(&panel, &cookie, group.id, "改名", json!([forged])).await?;
    ensure!(
        rejected == StatusCode::CONFLICT,
        "forged source: {rejected}"
    );
    let unchanged = group.listed(&panel, &cookie).await?;
    ensure!(
        unchanged["name"] == group.name.as_str()
            && unchanged["external_nodes"].as_array().map(Vec::len) == Some(1)
            && reference(&unchanged["external_nodes"][0], "follow_node") == saved,
        "{unchanged}"
    );
    ensure!(content(&panel, &user).await?.contains(FIRST));

    let other = panel.create_user(&cookie, "未授权用户").await?;
    ensure!(status(&panel, &other).await? == StatusCode::CONFLICT);
    let ready = preview(&panel, &cookie, uid).await?;
    ensure!(ready["external_nodes"] == 1 && ready["managed_nodes"] == 0);
    ensure!(
        ready["ready_nodes"][0]["kind"] == "external"
            && ready["available_formats"] == json!(["singbox", "clash"])
    );

    let cleared = group.save(&panel, &cookie, json!([])).await?;
    ensure!(cleared["external_nodes"] == json!([]), "{cleared}");
    panel
        .admin(
            Method::PATCH,
            &format!("{ROOT}/subscription-sources/{source}/nodes/{node}"),
            &cookie,
            Some(json!({"adopted":false,"settings_revision":1,"identity_epoch":1,"node_version_id":version,"metadata_revision":0})),
        )
        .await?
        .error_for_status()?;
    let adoption_revision: i64 = sqlx::query_scalar(
        "SELECT revision FROM singbox_node_metadata WHERE kind='external' AND id=$1",
    )
    .bind(node)
    .fetch_one(&pool)
    .await?;
    // An unadopted node is neither offered nor accepted.
    ensure!(candidates(&panel, &cookie).await?.is_empty());
    let mut unadopted = saved.clone();
    unadopted["metadata_revision"] = json!(adoption_revision);
    let (rejected, _) = group.put(&panel, &cookie, json!([unadopted])).await?;
    ensure!(
        rejected == StatusCode::CONFLICT,
        "unadopted node: {rejected}"
    );
    panel
        .admin(
            Method::PATCH,
            &format!("{ROOT}/subscription-sources/{source}/nodes/{node}"),
            &cookie,
            Some(json!({"adopted":true,"settings_revision":1,"identity_epoch":1,"node_version_id":version,"metadata_revision":adoption_revision})),
        )
        .await?
        .error_for_status()?;
    ensure!(status(&panel, &user).await? == StatusCode::CONFLICT);
    assign(&panel, &cookie, uid, "pinned").await?;
    ensure!(content(&panel, &user).await?.contains(FIRST));
    sqlx::query("UPDATE users SET deleted_at=1 WHERE id=$1")
        .bind(uid)
        .execute(&pool)
        .await?;
    ensure!(status(&panel, &user).await? == StatusCode::NOT_FOUND);
    Ok(())
}

#[sqlx::test]
async fn follow_and_pinned_versions_obey_identity_availability_and_failed_refresh(
    pool: PgPool,
) -> Result<()> {
    let (source, node) = source(&pool).await?;
    let panel = TestPanel::start(pool.clone()).await?;
    let cookie = panel.admin_cookie().await?;
    let follow = panel.create_user(&cookie, "跟随").await?;
    let pinned = panel.create_user(&cookie, "固定").await?;
    let (follow_group, _) = assign(&panel, &cookie, id(&follow)?, "follow_node").await?;
    let (pinned_group, _) = assign(&panel, &cookie, id(&pinned)?, "pinned").await?;
    version(&pool, source, node, SECOND).await?;
    ensure!(content(&panel, &follow).await?.contains(SECOND));
    ensure!(content(&panel, &pinned).await?.contains(FIRST));
    sqlx::query(
        "UPDATE singbox_subscription_sources SET last_error='fixture_fetch_failed' WHERE id=$1",
    )
    .bind(source)
    .execute(&pool)
    .await?;
    ensure!(content(&panel, &follow).await?.contains(SECOND));
    ensure!(
        follow_group.held(&panel, &cookie).await?[0]["source_last_error"] == "fixture_fetch_failed"
    );

    // A group that holds nothing yet can take the current version only while
    // the node is eligible.
    let probe = Group::create(&panel, &cookie, "新授权").await?;
    let fresh = reference(&first_candidate(&panel, &cookie).await?, "follow_node");
    probe.save(&panel, &cookie, json!([fresh.clone()])).await?;
    probe.save(&panel, &cookie, json!([])).await?;
    let transitions = [
        (
            "UPDATE singbox_external_nodes SET present=FALSE WHERE id=$1",
            "UPDATE singbox_external_nodes SET present=TRUE WHERE id=$1",
            node,
        ),
        (
            "UPDATE singbox_external_nodes SET adopted=FALSE WHERE id=$1",
            "UPDATE singbox_external_nodes SET adopted=TRUE WHERE id=$1",
            node,
        ),
        (
            "UPDATE singbox_external_nodes SET identity_unique=FALSE WHERE id=$1",
            "UPDATE singbox_external_nodes SET identity_unique=TRUE WHERE id=$1",
            node,
        ),
        (
            "UPDATE singbox_subscription_sources SET archived=TRUE WHERE id=$1",
            "UPDATE singbox_subscription_sources SET archived=FALSE WHERE id=$1",
            source,
        ),
        (
            "UPDATE singbox_subscription_sources SET identity_epoch=2 WHERE id=$1",
            "UPDATE singbox_subscription_sources SET identity_epoch=1 WHERE id=$1",
            source,
        ),
    ];
    for (disable, restore, target) in transitions {
        sqlx::query(disable).bind(target).execute(&pool).await?;
        for (user, group) in [(&follow, &follow_group), (&pinned, &pinned_group)] {
            ensure!(
                status(&panel, user).await? == StatusCode::CONFLICT,
                "{disable}"
            );
            let held = group.held(&panel, &cookie).await?;
            ensure!(
                held.len() == 1 && held[0]["available"] == false && held[0]["reason"].is_string(),
                "{disable}: {held:?}"
            );
        }
        let (rejected, _) = probe.put(&panel, &cookie, json!([fresh.clone()])).await?;
        ensure!(rejected == StatusCode::CONFLICT, "{disable}: {rejected}");
        sqlx::query(restore).bind(target).execute(&pool).await?;
    }
    ensure!(probe.held(&panel, &cookie).await?.is_empty());

    // A version from an outdated parser keeps serving existing bindings but
    // cannot be bound anew.
    parsed_version(&pool, source, node, THIRD, OUTDATED_PARSER).await?;
    ensure!(content(&panel, &follow).await?.contains(THIRD));
    ensure!(content(&panel, &pinned).await?.contains(FIRST));
    let outdated = first_candidate(&panel, &cookie).await?;
    ensure!(
        outdated["available"] == false && outdated["reason"] == "parser_update_required",
        "{outdated}"
    );
    let (rejected, _) = probe
        .put(
            &panel,
            &cookie,
            json!([reference(&outdated, "follow_node")]),
        )
        .await?;
    ensure!(
        rejected == StatusCode::CONFLICT,
        "outdated parser: {rejected}"
    );

    sqlx::query("INSERT INTO singbox_node_metadata(kind,id,enabled) VALUES('external',$1,FALSE)")
        .bind(node)
        .execute(&pool)
        .await?;
    ensure!(status(&panel, &follow).await? == StatusCode::CONFLICT);
    // The group keeps a binding it already holds even while the node is disabled.
    let same = reference(&follow_group.held(&panel, &cookie).await?[0], "follow_node");
    follow_group.save(&panel, &cookie, json!([same])).await?;
    let held = follow_group.held(&panel, &cookie).await?;
    ensure!(
        held[0]["available"] == false && held[0]["reason"] == "node_disabled",
        "{held:?}"
    );
    Ok(())
}

#[sqlx::test]
async fn catalog_alias_and_order_reach_group_external_subscriptions(pool: PgPool) -> Result<()> {
    let (_, first) = source(&pool).await?;
    let (_, second) = source(&pool).await?;
    let panel = TestPanel::start(pool.clone()).await?;
    let cookie = panel.admin_cookie().await?;
    let user = panel.create_user(&cookie, "目录订阅").await?;
    let uid = id(&user)?;
    let group = Group::for_user(&panel, &cookie, uid).await?;
    let references: Vec<_> = candidates(&panel, &cookie)
        .await?
        .iter()
        .map(|entry| reference(entry, "follow_node"))
        .collect();
    ensure!(references.len() == 2);
    group.save(&panel, &cookie, json!(references)).await?;
    join(&panel, &cookie, uid, group.id).await?;
    let before: Value = serde_json::from_str(&content(&panel, &user).await?)?;
    ensure!(
        before["outbounds"][0]["outbounds"]
            == json!([
                format!("external-node-{first} 外部"),
                format!("external-node-{second} 外部")
            ])
    );
    let catalog: Vec<Value> = panel
        .admin(Method::GET, &format!("{ROOT}/node-catalog"), &cookie, None)
        .await?
        .error_for_status()?
        .json()
        .await?;
    let alias = "香港 \"线路\" \\ A";
    let items: Vec<_> = catalog
        .iter()
        .map(|entry| {
            json!({
                "kind":"external", "id":entry["id"], "revision":entry["revision"],
                "name":alias, "sort_order":if entry["id"] == first { 20 } else { 10 }
            })
        })
        .collect();
    panel
        .admin(
            Method::PATCH,
            &format!("{ROOT}/node-catalog/batch"),
            &cookie,
            Some(json!({"items":items})),
        )
        .await?
        .error_for_status()?;
    let updated: Value = serde_json::from_str(&content(&panel, &user).await?)?;
    ensure!(
        updated["outbounds"][0]["outbounds"]
            == json!([
                format!("external-node-{second} {alias}"),
                format!("external-node-{first} {alias}")
            ])
    );
    ensure!(updated["outbounds"][1]["tag"] == format!("external-node-{second} {alias}"));
    ensure!(updated["outbounds"][1]["password"] == FIRST);
    // The group lists its nodes under the alias, in catalog order.
    let held = group.held(&panel, &cookie).await?;
    ensure!(
        held.iter()
            .map(|entry| entry["external_node_id"].clone())
            .collect::<Vec<_>>()
            == [json!(second), json!(first)]
            && held.iter().all(|entry| entry["name"] == alias),
        "{held:?}"
    );
    // Renaming changed each node's management revision: a draft read before it
    // is refused, and the refreshed one keeps both nodes.
    let (rejected, _) = group.put(&panel, &cookie, json!(references)).await?;
    ensure!(rejected == StatusCode::CONFLICT, "stale draft: {rejected}");
    let refreshed: Vec<_> = held
        .iter()
        .map(|entry| reference(entry, "follow_node"))
        .collect();
    group.save(&panel, &cookie, json!(refreshed)).await?;
    ensure!(content(&panel, &user).await? == serde_json::to_string_pretty(&updated)? + "\n");
    Ok(())
}

#[sqlx::test]
async fn mixed_subscription_keeps_managed_applied_health_and_entitlement_gates(
    pool: PgPool,
) -> Result<()> {
    source(&pool).await?;
    let panel = TestPanel::start(pool.clone()).await?;
    let cookie = panel.admin_cookie().await?;
    let server = panel.create_server(&cookie, "受管").await?;
    let managed = panel.create_node(&cookie, server, "受管入口").await?;
    let user = panel.create_user(&cookie, "混合订阅").await?;
    let uid = id(&user)?;
    panel.grant(&cookie, uid, id(&managed)?).await?;
    assign(&panel, &cookie, uid, "follow_node").await?;
    ensure!(user_groups(&panel, &cookie, uid).await?.len() == 2);
    let before: Value = serde_json::from_str(&content(&panel, &user).await?)?;
    ensure!(
        before["outbounds"][0]["outbounds"]
            .as_array()
            .context("selector")?
            .len()
            == 1
    );
    panel.publish_now().await?;
    sqlx::query("UPDATE server_module_status SET applied_rev=target_rev,healthy=TRUE")
        .execute(&pool)
        .await?;
    let ready: Value = serde_json::from_str(&content(&panel, &user).await?)?;
    ensure!(ready["outbounds"][0]["outbounds"] == json!(["node-1", "external-node-1 外部"]));
    sqlx::query("UPDATE server_module_status SET healthy=FALSE")
        .execute(&pool)
        .await?;
    let failed: Value = serde_json::from_str(&content(&panel, &user).await?)?;
    ensure!(
        failed["outbounds"][0]["outbounds"]
            .as_array()
            .context("selector")?
            .len()
            == 1
    );
    let package:Value=panel.admin(Method::POST,&format!("{ROOT}/package-groups"),&cookie,Some(json!({"name":"过期套餐","monthly_bytes":"1024","reset_day":1,"reset_hour":0,"reset_minute":0,"timezone":"UTC","duration_days":1}))).await?.error_for_status()?.json().await?;
    panel
        .admin(
            Method::POST,
            &format!("{ROOT}/users/{uid}/package"),
            &cookie,
            Some(json!({"package_group_id":id(&package)?,"request_id":uuid::Uuid::new_v4()})),
        )
        .await?
        .error_for_status()?;
    sqlx::query("UPDATE singbox_package_assignments SET starts_at=$2-172800,expires_at=$2-86400 WHERE user_id=$1").bind(uid).bind(apsp_protocol::now_timestamp()).execute(&pool).await?;
    ensure!(status(&panel, &user).await? == StatusCode::CONFLICT);
    let blocked = preview(&panel, &cookie, uid).await?;
    ensure!(
        blocked["status"] == "blocked"
            && blocked["content"].is_null()
            && blocked["ready_nodes"] == json!([])
    );
    Ok(())
}

#[sqlx::test]
async fn inactive_pinned_identity_is_retained_but_never_reissued(pool: PgPool) -> Result<()> {
    const MAX: i64 = 9_007_199_254_740_991;
    let (source, node) = source(&pool).await?;
    let panel = TestPanel::start(pool.clone()).await?;
    let cookie = panel.admin_cookie().await?;
    let user = panel.create_user(&cookie, "Pinned history").await?;
    let uid = id(&user)?;
    let (group, assigned) = assign(&panel, &cookie, uid, "pinned").await?;
    let first = reference(&assigned["external_nodes"][0], "pinned");
    // An old creation time, so that a re-created row could not match it by chance.
    sqlx::query("UPDATE singbox_policy_external_nodes SET created_at=1 WHERE group_id=$1")
        .bind(group.id)
        .execute(&pool)
        .await?;
    let row = || {
        sqlx::query_scalar::<_, Value>(
            "SELECT to_jsonb(p) FROM singbox_policy_external_nodes p WHERE group_id=$1",
        )
        .bind(group.id)
        .fetch_one(&pool)
    };
    let original = row().await?;
    version(&pool, source, node, SECOND).await?;
    sqlx::query("UPDATE singbox_subscription_sources SET last_error='TEST_ONLY cached fetch failure' WHERE id=$1")
        .bind(source).execute(&pool).await?;
    let other = panel.create_user(&cookie, "New current grant").await?;
    assign(&panel, &cookie, id(&other)?, "follow_node").await?;
    assert!(content(&panel, &other).await?.contains(SECOND));
    assert!(content(&panel, &user).await?.contains(FIRST));
    sqlx::query("INSERT INTO singbox_node_metadata(kind,id,enabled) VALUES('external',$1,FALSE)")
        .bind(node)
        .execute(&pool)
        .await?;
    let held = group.held(&panel, &cookie).await?;
    let retained_reference = reference(&held[0], "pinned");
    assert_eq!(
        retained_reference["node_version_id"],
        first["node_version_id"]
    );
    let kept = group
        .save(&panel, &cookie, json!([retained_reference.clone()]))
        .await?;
    assert_eq!(kept["external_nodes"][0]["available"], false);
    assert_eq!(
        row().await?,
        original,
        "retaining an inactive identity preserves its creation receipt"
    );
    let fresh = Group::create(&panel, &cookie, "No inactive issuance").await?;
    assert_eq!(
        fresh
            .put(&panel, &cookie, json!([retained_reference.clone()]))
            .await?
            .0,
        StatusCode::CONFLICT
    );
    let mut changed_mode = retained_reference.clone();
    changed_mode["update_mode"] = json!("follow_node");
    assert_eq!(
        group.put(&panel, &cookie, json!([changed_mode])).await?.0,
        StatusCode::CONFLICT
    );
    assert_eq!(status(&panel, &user).await?, StatusCode::CONFLICT);
    for (field, value) in [
        ("identity_epoch", json!(MAX + 1)),
        ("external_node_id", json!(MAX + 1)),
        ("metadata_revision", json!(-1)),
    ] {
        let mut unsafe_identity = retained_reference.clone();
        unsafe_identity[field] = value;
        assert_eq!(
            group
                .put(&panel, &cookie, json!([unsafe_identity]))
                .await?
                .0,
            StatusCode::BAD_REQUEST,
            "{field}"
        );
    }
    assert_eq!(row().await?, original);
    Ok(())
}

#[sqlx::test]
async fn overlapping_groups_give_one_binding_and_keep_the_node_until_the_last_goes(
    pool: PgPool,
) -> Result<()> {
    let (source, node) = source(&pool).await?;
    let panel = TestPanel::start(pool.clone()).await?;
    let cookie = panel.admin_cookie().await?;
    let user = panel.create_user(&cookie, "两个策略组").await?;
    let uid = id(&user)?;
    let original = first_candidate(&panel, &cookie).await?;
    let pinned = Group::create(&panel, &cookie, "固定首版").await?;
    pinned
        .save(&panel, &cookie, json!([reference(&original, "pinned")]))
        .await?;
    let follow = Group::create(&panel, &cookie, "跟随").await?;
    follow
        .save(
            &panel,
            &cookie,
            json!([reference(&original, "follow_node")]),
        )
        .await?;
    let second = version(&pool, source, node, SECOND).await?;
    let newer = Group::create(&panel, &cookie, "固定次版").await?;
    newer
        .save(
            &panel,
            &cookie,
            json!([reference(
                &first_candidate(&panel, &cookie).await?,
                "pinned"
            )]),
        )
        .await?;
    let third = version(&pool, source, node, THIRD).await?;

    // Three groups hold the node; the user gets it once, through the binding
    // that follows the node.
    set_groups(&panel, &cookie, uid, &[pinned.id, follow.id, newer.id]).await?;
    let rendered: Value = serde_json::from_str(&content(&panel, &user).await?)?;
    ensure!(
        rendered["outbounds"][0]["outbounds"] == json!([format!("external-node-{node} 外部")]),
        "{rendered}"
    );
    let text = rendered.to_string();
    ensure!(text.contains(THIRD) && !text.contains(FIRST) && !text.contains(SECOND));
    let ready = preview(&panel, &cookie, uid).await?;
    ensure!(
        ready["external_granted_nodes"] == 1 && ready["external_nodes"] == 1,
        "{ready}"
    );
    let bindings = authorizations(&panel, &cookie, uid).await?;
    ensure!(
        bindings.len() == 1
            && bindings[0]["update_mode"] == "follow_node"
            && bindings[0]["node_version_id"] == original["node_version_id"]
            && bindings[0]["resolved_version_id"] == third,
        "{bindings:?}"
    );

    // Leaving the following group keeps the node, now through the newest pin.
    set_groups(&panel, &cookie, uid, &[pinned.id, newer.id]).await?;
    let text = content(&panel, &user).await?;
    ensure!(text.contains(SECOND) && !text.contains(FIRST) && !text.contains(THIRD));
    let bindings = authorizations(&panel, &cookie, uid).await?;
    ensure!(
        bindings.len() == 1
            && bindings[0]["update_mode"] == "pinned"
            && bindings[0]["node_version_id"] == second,
        "{bindings:?}"
    );

    // Emptying a group the user is still in leaves the other group's binding.
    newer.save(&panel, &cookie, json!([])).await?;
    ensure!(content(&panel, &user).await?.contains(FIRST));
    set_groups(&panel, &cookie, uid, &[follow.id]).await?;
    ensure!(content(&panel, &user).await?.contains(THIRD));
    set_groups(&panel, &cookie, uid, &[]).await?;
    ensure!(status(&panel, &user).await? == StatusCode::CONFLICT);
    ensure!(authorizations(&panel, &cookie, uid).await?.is_empty());
    Ok(())
}
