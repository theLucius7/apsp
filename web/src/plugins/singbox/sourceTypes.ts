// Why an adopted external node cannot be assigned right now, as the node catalog reports it.
const reasons: Record<string, string> = {
  source_deleted: '来源已删除',
  source_archived: '来源已归档，不能用于新链路',
  source_replaced: '来源已更换，需要重新选点；原链路保留旧版本',
  node_missing: '所选节点本次缺失，原链路保留旧版本',
  ambiguous_node_identity: '同一端点存在多个身份，不能自动匹配原节点',
  unsupported_configuration: '当前节点配置不可用',
}

export function sourceReason(reason: string | null | undefined): string {
  return reason ? reasons[reason] ?? '订阅内容或节点参数校验未通过' : ''
}
