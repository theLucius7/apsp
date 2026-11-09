use crate::error::{ApiError, ApiResult};

/// Both node deletion routes call this under the topology and server locks.
pub(crate) async fn ensure_unreferenced_on(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    id: i64,
) -> ApiResult<()> {
    // A damaged selected version cannot erase saved hop ownership before
    // confirmed retirement. Complete owners protect only their selected versions.
    let referenced: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM singbox_policy_nodes WHERE node_id=$1) OR EXISTS(SELECT 1 FROM singbox_chains c LEFT JOIN singbox_ordered_chain_hops h ON h.chain_id=c.id AND (h.generation=ANY(ARRAY[c.desired_generation,c.applied_generation,c.candidate_generation,c.recovery_generation]) OR EXISTS(SELECT 1 FROM unnest(ARRAY[c.desired_generation,c.applied_generation,c.candidate_generation,c.recovery_generation]) AS selected(generation) WHERE selected.generation IS NOT NULL AND NOT EXISTS(SELECT 1 FROM singbox_ordered_chain_versions v WHERE v.chain_id=c.id AND v.generation=selected.generation))) WHERE c.path_kind='ordered' AND (c.deleted_at IS NULL OR c.phase<>'retired') AND (c.entry_node_id=$1 OR h.managed_node_id=$1))")
        .bind(id).fetch_one(&mut **tx).await?;
    if referenced {
        return Err(ApiError::Conflict(
            "节点仍被策略或链路引用，请先通过对应资源接口解除引用".into(),
        ));
    }
    Ok(())
}
