export type ExternalReference = { external_node_id: number; source_id: number; identity_epoch: number; node_version_id: number; update_mode: 'follow_node' | 'pinned'; metadata_revision: number }
export type ExternalEntry = ExternalReference & { name: string; source_name: string; protocol: string; server: string; port: number; available: boolean; reason: string | null; current_version_id: number | null; resolved_version_id: number | null; source_last_error: string | null }
export const externalReference = (entry: ExternalReference): ExternalReference => ({ external_node_id: entry.external_node_id, source_id: entry.source_id, identity_epoch: entry.identity_epoch, node_version_id: entry.node_version_id, update_mode: entry.update_mode, metadata_revision: entry.metadata_revision })
export function sameExternalReference(left: ExternalReference, right: ExternalReference) {
  return left.external_node_id === right.external_node_id && left.source_id === right.source_id && left.identity_epoch === right.identity_epoch && left.node_version_id === right.node_version_id && left.update_mode === right.update_mode && left.metadata_revision === right.metadata_revision
}
export function eligibleExternalSelection(entry: ExternalEntry) {
  return entry.available && entry.reason === null && entry.current_version_id === entry.node_version_id && entry.resolved_version_id === entry.node_version_id
}
