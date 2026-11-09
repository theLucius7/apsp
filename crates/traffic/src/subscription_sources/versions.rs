//! External node versions as compiled outbounds, read through
//! `singbox_external_version_configs`.

use crate::error::ApiResult;
use apsp_compiler::external::{ExternalOutbound, NormalizedOutbound};
use serde_json::Value;
use sqlx::{FromRow, PgConnection};
use std::collections::BTreeMap;

/// The parser version of the versions imported from the numbered sources (2026-11-04).
pub const IMPORTED_PARSER_VERSION: &str = "apsp-subscriptions-2";

/// A version read through `singbox_external_version_configs`: a typed outbound, or the
/// original outbound of a version imported from the numbered sources on 2026-11-04.
#[derive(FromRow)]
pub struct Version {
    pub id: i64,
    pub external_node_id: i64,
    pub source_id: i64,
    pub identity_epoch: i64,
    pub parser_version: String,
    pub ordered: bool,
    pub config_json: Option<Value>,
    pub config_sha256: Option<String>,
    pub normalized_config: Option<Value>,
    pub content_digest: Option<String>,
}
impl Version {
    pub fn outbound(&self) -> Option<ExternalOutbound> {
        if let (Some(config), Some(sha256)) = (&self.config_json, &self.config_sha256) {
            let value = ExternalOutbound(config.clone());
            value.capabilities().ok()?;
            let serialized = serde_json::to_vec(&value).ok()?;
            return (crate::subscription_parser::hex_digest(&serialized) == *sha256)
                .then_some(value);
        }
        // Typed outbounds are checked against their stored content digest first.
        let typed: NormalizedOutbound =
            serde_json::from_value(self.normalized_config.clone()?).ok()?;
        let serialized = serde_json::to_vec(&typed).ok()?;
        if crate::subscription_parser::hex_digest(&serialized) != *self.content_digest.as_ref()? {
            return None;
        }
        let value = ExternalOutbound::from_normalized(&typed).ok()?;
        value.capabilities().ok()?;
        Some(value)
    }

    /// New grants require the parser that currently produces this kind of version. An
    /// imported version is still served from its original outbound, so it keeps the parser
    /// version it was imported with.
    pub fn current_parser(&self) -> bool {
        self.parser_version
            == if self.ordered && self.config_json.is_none() {
                crate::subscription_parser::PARSER_VERSION
            } else {
                IMPORTED_PARSER_VERSION
            }
    }
}

pub async fn versions(
    connection: &mut PgConnection,
    ids: Vec<i64>,
) -> ApiResult<BTreeMap<i64, Version>> {
    let rows:Vec<Version> = sqlx::query_as("SELECT id,external_node_id,source_id,identity_epoch,parser_version,ordered,config_json,config_sha256,normalized_config,content_digest FROM singbox_external_version_configs WHERE id=ANY($1)")
        .bind(ids).fetch_all(connection).await?;
    Ok(rows.into_iter().map(|row| (row.id, row)).collect())
}
