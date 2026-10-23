//! Copies numbered sources into ordered sources without changing what users,
//! the catalog or devices receive.
//!
//! Ids stay stable: imported nodes and versions keep their numbered ids as
//! public ids, and each imported version keeps its original outbound
//! (`legacy_config`) next to the typed form, so rendered subscriptions keep
//! their bytes. Only the revisions that something still reads are imported.

use super::Report;
use crate::{
    error::{ApiError, ApiResult},
    subscription_parser::{self, FormatHint, ParseStatus},
    subscription_sources::{
        identity_keys,
        models::{SourceFailure, SourceInput},
    },
};
use apsp_protocol::now_timestamp;
use serde_json::{Value, json};
use sqlx::{FromRow, PgConnection, Postgres, QueryBuilder};
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;

/// Ordered sources keep at most this many live sources (see `mutations::create`).
const LIVE_SOURCE_LIMIT: i64 = 128;
const MAX_MEMBERS: usize = subscription_parser::MAX_NODES;
const CHUNK: usize = 1000;

// Secrets: deliberately not Debug.
#[derive(FromRow)]
struct Source {
    id: i64,
    name: String,
    kind: String,
    secret_url: Option<String>,
    secret_authorization: Option<String>,
    secret_content: Option<String>,
    source_host: Option<String>,
    settings_revision: i64,
    identity_epoch: i64,
    refresh_interval_seconds: i64,
    archived: bool,
    deleted_at: Option<i64>,
    current_revision_id: Option<i64>,
    last_attempt_at: Option<i64>,
    last_success_at: Option<i64>,
    next_refresh_at: Option<i64>,
    last_error: Option<String>,
    created_at: i64,
    auto_refresh: bool,
    user_agent: String,
    traffic: Value,
    changes: Value,
}

#[derive(FromRow)]
struct Revision {
    id: i64,
    source_id: i64,
    settings_revision: i64,
    identity_epoch: i64,
    parser_version: String,
    body_sha256: String,
    format: String,
    supported_count: i32,
    unsupported_count: i32,
    ambiguous_count: i64,
    fetched_at: i64,
}

#[derive(FromRow)]
struct Node {
    id: i64,
    source_id: i64,
    identity_epoch: i64,
    identity_key: String,
    present: bool,
    identity_unique: bool,
    current_version_id: Option<i64>,
    last_seen_revision_id: Option<i64>,
    adopted: bool,
}

#[derive(FromRow)]
struct Version {
    id: i64,
    external_node_id: i64,
    source_revision_id: i64,
    name: String,
    config_json: Value,
    config_sha256: String,
}

/// One numbered version in its ordered form.
struct Converted {
    uuid: Uuid,
    a: Version,
    normalized: Option<Value>,
    digest: Option<String>,
    fingerprint: Option<String>,
    preview: Value,
    capabilities: Value,
    reasons: Value,
    supported: bool,
}

struct Imported {
    uuid: Uuid,
    a: Node,
    key: Option<String>,
    state: &'static str,
}

struct Member {
    node: Uuid,
    version: Uuid,
    preview: Value,
    state: &'static str,
}

/// Everything `apply` writes, computed under the migration locks.
pub(super) struct Plan {
    pub report: Report,
    sources: Vec<Source>,
    revisions: Vec<(Revision, Uuid, &'static str)>,
    nodes: Vec<Imported>,
    versions: Vec<Converted>,
    members: BTreeMap<i64, Vec<Member>>,
}

fn format(value: &str) -> Option<&'static str> {
    Some(match value {
        "uri" => "uri_list",
        "base64_uri" => "base64_uri_list",
        "sing_box_json" => "sing_box_json",
        "clash_yaml" => "clash_yaml",
        _ => return None,
    })
}

fn convert(a: Version) -> ApiResult<Converted> {
    let parsed = subscription_parser::import_outbound(a.config_json.clone(), 0, &a.name);
    let supported = parsed.preview.parse_status == ParseStatus::Supported
        && parsed.outbound.is_some()
        && parsed.content_digest.is_some();
    let normalized = match (&parsed.outbound, supported) {
        (Some(outbound), true) => {
            Some(serde_json::to_value(outbound).map_err(anyhow::Error::from)?)
        }
        _ => None,
    };
    let capabilities = parsed
        .outbound
        .as_ref()
        .map(|outbound| json!({"tcp":outbound.tcp(),"udp":outbound.udp()}))
        .unwrap_or_else(|| json!({"tcp":false,"udp":false}));
    let reasons: Vec<&str> = parsed
        .preview
        .unsupported_reasons
        .iter()
        .map(|reason| reason.message.as_str())
        .collect();
    Ok(Converted {
        uuid: Uuid::new_v4(),
        normalized,
        digest: if supported {
            parsed.content_digest
        } else {
            None
        },
        fingerprint: parsed.identity_fingerprint.filter(|_| supported),
        preview: json!(parsed.preview),
        capabilities,
        reasons: json!(reasons),
        supported,
        a,
    })
}

/// Identity keys that ordered sources will derive for the same nodes:
/// provider ids keep their key, endpoint keys become the typed fingerprint.
/// A node whose new key would be shared keeps its numbered key instead, and
/// is then matched by a later refresh only through its provider id.
fn identities(nodes: &[Node], versions: &BTreeMap<i64, &Converted>) -> (Vec<Option<String>>, i64) {
    let mut keys: Vec<Option<String>> = nodes
        .iter()
        .map(|node| {
            if !node.identity_unique {
                return None;
            }
            if node.identity_key.starts_with("provider:") {
                return Some(node.identity_key.clone());
            }
            Some(
                node.current_version_id
                    .and_then(|id| versions.get(&id))
                    .and_then(|version| version.fingerprint.as_ref())
                    .map(|fingerprint| format!("fingerprint:{fingerprint}"))
                    .unwrap_or_else(|| node.identity_key.clone()),
            )
        })
        .collect();
    let mut groups = BTreeMap::<(i64, i64, String), Vec<usize>>::new();
    for (index, (node, key)) in nodes.iter().zip(&keys).enumerate() {
        if let Some(key) = key {
            groups
                .entry((node.source_id, node.identity_epoch, key.clone()))
                .or_default()
                .push(index);
        }
    }
    let mut collisions = 0;
    for indexes in groups.into_values().filter(|group| group.len() > 1) {
        for index in indexes {
            keys[index] = Some(nodes[index].identity_key.clone());
            collisions += 1;
        }
    }
    (keys, collisions)
}

/// For inline sources the stored content can be parsed again: counts present
/// nodes whose imported key the ordered parser would not produce, which a
/// later content update would then treat as missing.
fn inline_changes(sources: &[Source], nodes: &[Imported], report: &mut Report) {
    for source in sources {
        let (Some(content), None) = (&source.secret_content, source.deleted_at) else {
            continue;
        };
        let Ok(parsed) =
            subscription_parser::parse_subscription(content.as_bytes(), FormatHint::Auto)
        else {
            report.inline_sources_not_reparsed += 1;
            continue;
        };
        let produced: BTreeSet<String> = identity_keys(&parsed).into_iter().flatten().collect();
        report.inline_identity_changes += nodes
            .iter()
            .filter(|node| {
                node.a.source_id == source.id
                    && node.a.identity_epoch == source.identity_epoch
                    && node.a.present
                    && node.a.last_seen_revision_id == source.current_revision_id
                    && node.key.as_ref().is_some_and(|key| !produced.contains(key))
            })
            .count() as i64;
    }
}

pub(super) async fn plan(tx: &mut PgConnection) -> ApiResult<Plan> {
    let mut report = Report {
        migrated: super::migrated(tx).await?,
        ..Report::default()
    };
    if report.migrated {
        report.blockers.push("already_migrated".into());
    }
    let sources: Vec<Source> = sqlx::query_as("SELECT id,name,kind,secret_url,secret_authorization,secret_content,source_host,settings_revision,identity_epoch,refresh_interval_seconds,archived,deleted_at,current_revision_id,last_attempt_at,last_success_at,next_refresh_at,last_error,created_at,auto_refresh,user_agent,traffic,changes FROM singbox_subscription_sources ORDER BY id")
        .fetch_all(&mut *tx).await?;
    let nodes: Vec<Node> = sqlx::query_as("SELECT id,source_id,identity_epoch,identity_key,present,identity_unique,current_version_id,last_seen_revision_id,adopted FROM singbox_external_nodes ORDER BY id")
        .fetch_all(&mut *tx).await?;
    // Versions still read: each node's current version, and those that user
    // grants and mixed-chain hops are bound to.
    let versions: Vec<Version> = sqlx::query_as("SELECT id,external_node_id,source_revision_id,name,config_json,config_sha256 FROM singbox_external_node_versions WHERE id IN (SELECT current_version_id FROM singbox_external_nodes UNION SELECT node_version_id FROM singbox_external_accesses UNION SELECT external_version_id FROM singbox_chain_hops) ORDER BY id")
        .fetch_all(&mut *tx).await?;
    let referenced: BTreeSet<i64> = sqlx::query_scalar("SELECT node_version_id FROM singbox_external_accesses UNION SELECT external_version_id FROM singbox_chain_hops WHERE external_version_id IS NOT NULL")
        .fetch_all(&mut *tx).await?.into_iter().collect();
    // Revisions still read: those of the imported versions, each source's
    // current one, each node's last listing, and the latest of every epoch
    // (which decides whether a node is present).
    let revisions: Vec<Revision> = sqlx::query_as("SELECT r.id,r.source_id,r.settings_revision,r.identity_epoch,r.parser_version,r.body_sha256,r.format,r.supported_count,r.unsupported_count,(SELECT COUNT(*) FROM jsonb_array_elements(r.rejected_nodes) e WHERE e->>'reason'='ambiguous_node_identity') AS ambiguous_count,r.fetched_at FROM singbox_source_revisions r WHERE r.id IN (SELECT source_revision_id FROM singbox_external_node_versions WHERE id=ANY($1) UNION SELECT current_revision_id FROM singbox_subscription_sources UNION SELECT last_seen_revision_id FROM singbox_external_nodes UNION SELECT MAX(id) FROM singbox_source_revisions GROUP BY source_id,identity_epoch) ORDER BY r.id")
        .bind(versions.iter().map(|version| version.id).collect::<Vec<_>>())
        .fetch_all(&mut *tx).await?;
    let counts: (i64, i64, i64, i64) = sqlx::query_as("SELECT (SELECT COUNT(*) FROM singbox_external_accesses),(SELECT COUNT(*) FROM singbox_source_jobs WHERE state IN ('queued','running')),(SELECT COUNT(*) FROM singbox_source_previews),(SELECT COUNT(*) FROM singbox_ordered_subscription_sources WHERE deleted_at IS NULL)")
        .fetch_one(&mut *tx).await?;
    let ambiguous_with_access: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM singbox_external_accesses a JOIN singbox_external_nodes n ON n.id=a.external_node_id WHERE NOT n.identity_unique")
        .fetch_one(&mut *tx).await?;
    let mixed_chains: i64 = sqlx::query_scalar("SELECT COUNT(DISTINCT h.chain_id) FROM singbox_chain_hops h JOIN singbox_chains c ON c.id=h.chain_id WHERE h.source_id IS NOT NULL AND c.deleted_at IS NULL")
        .fetch_one(&mut *tx).await?;

    report.numbered_sources = sources.len() as i64;
    report.numbered_sources_deleted =
        sources.iter().filter(|s| s.deleted_at.is_some()).count() as i64;
    report.numbered_nodes = nodes.len() as i64;
    report.revisions_to_import = revisions.len() as i64;
    report.versions_to_import = versions.len() as i64;
    (
        report.external_accesses,
        report.active_numbered_jobs,
        report.pending_numbered_previews,
        report.live_sources_after_merge,
    ) = counts;
    report.live_sources_after_merge +=
        sources.iter().filter(|s| s.deleted_at.is_none()).count() as i64;
    report.ambiguous_nodes_with_access = ambiguous_with_access;
    report.mixed_chains_with_subscription_hops = mixed_chains;

    let mut converted = Vec::with_capacity(versions.len());
    for version in versions {
        converted.push(convert(version)?);
    }
    report.versions_not_normalized = converted.iter().filter(|v| !v.supported).count() as i64;
    report.referenced_versions_not_normalized = converted
        .iter()
        .filter(|v| !v.supported && referenced.contains(&v.a.id))
        .count() as i64;
    let by_id: BTreeMap<i64, &Converted> = converted.iter().map(|v| (v.a.id, v)).collect();
    let (keys, collisions) = identities(&nodes, &by_id);
    report.identity_collisions = collisions;
    let imported: Vec<Imported> = nodes
        .into_iter()
        .zip(keys)
        .map(|(a, key)| Imported {
            uuid: Uuid::new_v4(),
            state: if key.is_some() { "unique" } else { "ambiguous" },
            key,
            a,
        })
        .collect();
    inline_changes(&sources, &imported, &mut report);

    let mut planned = Vec::with_capacity(revisions.len());
    for revision in revisions {
        match format(&revision.format) {
            Some(format)
                if revision.body_sha256.len() == 64
                    && revision
                        .body_sha256
                        .bytes()
                        .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) =>
            {
                planned.push((revision, Uuid::new_v4(), format));
            }
            _ => report
                .blockers
                .push(format!("revision_not_importable:{}", revision.id)),
        }
    }
    // Membership of each imported revision: nodes it listed last, and nodes
    // whose imported version it produced.
    let revision_ids: BTreeSet<i64> = planned.iter().map(|(r, ..)| r.id).collect();
    let mut members = BTreeMap::<i64, BTreeMap<i64, Member>>::new();
    let node_uuid: BTreeMap<i64, Uuid> = imported.iter().map(|n| (n.a.id, n.uuid)).collect();
    for node in &imported {
        let (Some(revision), Some(version)) = (
            node.a.last_seen_revision_id,
            node.a.current_version_id.and_then(|id| by_id.get(&id)),
        ) else {
            continue;
        };
        if revision_ids.contains(&revision) {
            members.entry(revision).or_default().insert(
                node.a.id,
                Member {
                    node: node.uuid,
                    version: version.uuid,
                    preview: version.preview.clone(),
                    state: node.state,
                },
            );
        }
    }
    for version in &converted {
        if let Some(node) = node_uuid.get(&version.a.external_node_id) {
            members
                .entry(version.a.source_revision_id)
                .or_default()
                .entry(version.a.external_node_id)
                .or_insert_with(|| Member {
                    node: *node,
                    version: version.uuid,
                    preview: version.preview.clone(),
                    state: "unique",
                });
        }
    }
    let members: BTreeMap<i64, Vec<Member>> = members
        .into_iter()
        .map(|(revision, nodes)| (revision, nodes.into_values().collect()))
        .collect();
    if members.values().any(|nodes| nodes.len() > MAX_MEMBERS) {
        report.blockers.push("revision_membership_limit".into());
    }

    if report.live_sources_after_merge > LIVE_SOURCE_LIMIT {
        report.warnings.push("live_source_limit_exceeded".into());
    }
    for (count, warning) in [
        (
            report.identity_collisions,
            "identity_collisions_keep_numbered_keys",
        ),
        (report.versions_not_normalized, "versions_not_normalized"),
        (
            report.referenced_versions_not_normalized,
            "referenced_versions_not_normalized",
        ),
        (
            report.ambiguous_nodes_with_access,
            "ambiguous_nodes_with_access",
        ),
        (report.inline_identity_changes, "inline_identity_changes"),
        (
            report.inline_sources_not_reparsed,
            "inline_sources_not_reparsed",
        ),
        (
            report.mixed_chains_with_subscription_hops,
            "mixed_chains_keep_numbered_versions",
        ),
    ] {
        if count > 0 {
            report.warnings.push(warning.into());
        }
    }
    Ok(Plan {
        report,
        sources,
        revisions: planned,
        nodes: imported,
        versions: converted,
        members,
    })
}

fn input(source: &Source) -> Value {
    if source.deleted_at.is_some() {
        return json!({});
    }
    match (&source.secret_url, &source.secret_content) {
        (Some(url), _) => json!(SourceInput::Url {
            url: url.clone(),
            auth_headers: source
                .secret_authorization
                .iter()
                .map(|value| ("authorization".to_owned(), value.clone()))
                .collect(),
        }),
        (None, Some(content)) => json!(SourceInput::Inline {
            content: content.clone()
        }),
        (None, None) => json!({}),
    }
}

pub(super) async fn apply(tx: &mut PgConnection, plan: Plan) -> ApiResult<Report> {
    let now = now_timestamp();
    let Plan {
        report,
        sources,
        revisions,
        nodes,
        versions,
        members,
    } = plan;
    // Numbered work stops: queued and running jobs end, previews are dropped.
    sqlx::query("UPDATE singbox_source_jobs SET state='cancelled',phase='finished',finished_at=$1 WHERE state IN ('queued','running')")
        .bind(now).execute(&mut *tx).await?;
    sqlx::query("DELETE FROM singbox_source_previews")
        .execute(&mut *tx)
        .await?;

    let mut source_map = BTreeMap::new();
    for source in &sources {
        let live = source.deleted_at.is_none() && !source.archived;
        let url = source.kind == "url";
        let error = source.last_error.as_deref().map(|code| {
            json!(SourceFailure::new(
                "done",
                code,
                "迁移前最近一次更新失败，详见原数字编号来源的错误代码"
            ))
        });
        let id: i64 = sqlx::query_scalar("INSERT INTO singbox_ordered_subscription_sources(name,kind,host,input_config,settings_revision,identity_epoch,archived,deleted_at,refresh_interval_secs,next_refresh_at,last_attempt_at,last_success_at,last_error,created_at,updated_at,user_agent,auto_refresh,traffic,changes) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19) RETURNING id")
            .bind(&source.name)
            .bind(&source.kind)
            .bind(source.source_host.as_ref().filter(|_| source.deleted_at.is_none()))
            .bind(input(source))
            .bind(source.settings_revision)
            .bind(source.identity_epoch)
            .bind(source.archived)
            .bind(source.deleted_at)
            .bind(if url { source.refresh_interval_seconds } else { 0 })
            .bind((url && live && source.auto_refresh).then(|| source.next_refresh_at.unwrap_or(now)))
            .bind(source.last_attempt_at)
            .bind(source.last_success_at)
            .bind(error)
            .bind(source.created_at)
            .bind(now)
            .bind(&source.user_agent)
            .bind(source.auto_refresh)
            .bind(&source.traffic)
            .bind(&source.changes)
            .fetch_one(&mut *tx)
            .await?;
        sqlx::query("INSERT INTO singbox_source_id_map(a_source_id,b_source_id) VALUES($1,$2)")
            .bind(source.id)
            .bind(id)
            .execute(&mut *tx)
            .await?;
        source_map.insert(source.id, id);
    }
    // Secrets must arrive byte for byte; compared in the database, never printed.
    let altered: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM singbox_source_id_map m JOIN singbox_subscription_sources a ON a.id=m.a_source_id JOIN singbox_ordered_subscription_sources b ON b.id=m.b_source_id WHERE a.deleted_at IS NULL AND NOT (b.input_config->>'url' IS NOT DISTINCT FROM a.secret_url AND b.input_config->>'content' IS NOT DISTINCT FROM a.secret_content AND b.input_config->'auth_headers'->>'authorization' IS NOT DISTINCT FROM a.secret_authorization)")
        .fetch_one(&mut *tx).await?;
    if altered > 0 {
        return Err(ApiError::Internal(anyhow::anyhow!(
            "source secrets changed while copying"
        )));
    }

    let mut revision_map = BTreeMap::new();
    for (revision, uuid, format) in &revisions {
        let source = source_map[&revision.source_id];
        let job = Uuid::new_v4();
        sqlx::query("INSERT INTO singbox_subscription_source_jobs(id,source_id,settings_revision,identity_epoch,parser_version,status,stage,created_at,started_at,finished_at) VALUES($1,$2,$3,$4,$5,'succeeded','done',$6,$6,$6)")
            .bind(job).bind(source).bind(revision.settings_revision).bind(revision.identity_epoch).bind(&revision.parser_version).bind(revision.fetched_at).execute(&mut *tx).await?;
        let counts = json!({
            "supported": revision.supported_count,
            "unsupported": revision.unsupported_count,
            "ambiguous": revision.ambiguous_count,
            "missing": 0,
        });
        sqlx::query("INSERT INTO singbox_subscription_source_revisions(id,source_id,job_id,settings_revision,identity_epoch,parser_version,format,raw_digest,parsed_at,counts,warnings) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,'[]'::jsonb)")
            .bind(uuid).bind(source).bind(job).bind(revision.settings_revision).bind(revision.identity_epoch).bind(&revision.parser_version).bind(format).bind(&revision.body_sha256).bind(revision.fetched_at).bind(counts).execute(&mut *tx).await?;
        sqlx::query(
            "UPDATE singbox_subscription_source_jobs SET source_revision_id=$2 WHERE id=$1",
        )
        .bind(job)
        .bind(uuid)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "INSERT INTO singbox_source_revision_map(a_revision_id,b_revision_id) VALUES($1,$2)",
        )
        .bind(revision.id)
        .bind(uuid)
        .execute(&mut *tx)
        .await?;
        revision_map.insert(revision.id, *uuid);
    }

    for chunk in nodes.chunks(CHUNK) {
        let mut query = QueryBuilder::<Postgres>::new(
            "INSERT INTO singbox_ordered_external_nodes(id,source_id,identity_epoch,identity_key,identity_state,created_at,adopted,public_id) ",
        );
        query.push_values(chunk, |mut row, node| {
            row.push_bind(node.uuid)
                .push_bind(source_map[&node.a.source_id])
                .push_bind(node.a.identity_epoch)
                .push_bind(&node.key)
                .push_bind(node.state)
                .push_bind(now)
                .push_bind(node.a.adopted)
                .push_bind(node.a.id);
        });
        query.build().execute(&mut *tx).await?;
    }
    let node_uuid: BTreeMap<i64, Uuid> = nodes.iter().map(|n| (n.a.id, n.uuid)).collect();
    for chunk in versions.chunks(CHUNK) {
        let mut query = QueryBuilder::<Postgres>::new(
            "INSERT INTO singbox_ordered_external_node_versions(id,node_id,source_revision_id,normalized_config,content_digest,public_preview,capabilities,supported,reasons,public_id,legacy_config,legacy_config_sha256) ",
        );
        query.push_values(chunk, |mut row, version| {
            row.push_bind(version.uuid)
                .push_bind(node_uuid[&version.a.external_node_id])
                .push_bind(revision_map[&version.a.source_revision_id])
                .push_bind(&version.normalized)
                .push_bind(&version.digest)
                .push_bind(&version.preview)
                .push_bind(&version.capabilities)
                .push_bind(version.supported)
                .push_bind(&version.reasons)
                .push_bind(version.a.id)
                .push_bind(&version.a.config_json)
                .push_bind(&version.a.config_sha256);
        });
        query.build().execute(&mut *tx).await?;
    }
    let rows: Vec<(Uuid, i32, &Member)> = members
        .iter()
        .flat_map(|(revision, nodes)| {
            let uuid = revision_map[revision];
            nodes
                .iter()
                .enumerate()
                .map(move |(ordinal, member)| (uuid, ordinal as i32, member))
        })
        .collect();
    for chunk in rows.chunks(CHUNK) {
        let mut query = QueryBuilder::<Postgres>::new(
            "INSERT INTO singbox_subscription_revision_nodes(source_revision_id,ordinal,node_id,version_id,public_preview,identity_state) ",
        );
        query.push_values(chunk, |mut row, (revision, ordinal, member)| {
            let mut preview = member.preview.clone();
            preview["ordinal"] = json!(ordinal);
            row.push_bind(*revision)
                .push_bind(*ordinal)
                .push_bind(member.node)
                .push_bind(member.version)
                .push_bind(preview)
                .push_bind(member.state);
        });
        query.build().execute(&mut *tx).await?;
    }
    let version_uuid: BTreeMap<i64, Uuid> = versions.iter().map(|v| (v.a.id, v.uuid)).collect();
    for node in &nodes {
        sqlx::query("UPDATE singbox_ordered_external_nodes SET latest_version=$2,last_seen_revision=$3 WHERE id=$1")
            .bind(node.uuid)
            .bind(node.a.current_version_id.and_then(|id| version_uuid.get(&id)))
            .bind(node.a.last_seen_revision_id.and_then(|id| revision_map.get(&id)))
            .execute(&mut *tx)
            .await?;
    }
    for source in &sources {
        if let Some(revision) = source
            .current_revision_id
            .and_then(|id| revision_map.get(&id))
        {
            sqlx::query("UPDATE singbox_ordered_subscription_sources SET current_success_revision=$2 WHERE id=$1")
                .bind(source_map[&source.id])
                .bind(revision)
                .execute(&mut *tx)
                .await?;
        }
    }
    // Grants name their node by public id, unchanged; only the source moves.
    sqlx::query("UPDATE singbox_external_accesses a SET source_id=m.b_source_id FROM singbox_source_id_map m WHERE a.source_id=m.a_source_id")
        .execute(&mut *tx)
        .await?;
    let mut report = report;
    report.migrated = true;
    sqlx::query("UPDATE singbox_source_migration SET migrated_at=$1,report=$2")
        .bind(now)
        .bind(json!(report))
        .execute(&mut *tx)
        .await?;
    Ok(report)
}
