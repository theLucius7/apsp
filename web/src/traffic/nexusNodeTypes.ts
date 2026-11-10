export type NexusNode = {
  id: number; source_id: string; legacy_node_id: number; legacy_machine_id: number; legacy_host_id: number
  server_id: number; server_name: string; machine_name: string; machine_active: boolean
  name: string; protocol: string; public_host: string; public_port: string; listen_port: string
  rate: string; enabled: boolean; visible: boolean; sort_order: number; tags: string[]; parent_id: number | null
  groups: { id: number; name: string }[]; routes: { id: number; name: string }[]
  protocol_summary: Record<string, string | number | boolean>; custom_route_count: number; custom_outbound_count: number
  timed_rate_enabled: boolean; timed_rate_count: number; certificate_saved: boolean; configuration_saved: true
  native_node_id?: number | null; state: 'pending_agent' | 'prepared' | 'applying' | 'applied' | 'paused'; exported_at: number; imported_at: number
}

const object = (value: unknown): value is Record<string, unknown> => typeof value === 'object' && value !== null && !Array.isArray(value)
const id = (value: unknown) => Number.isSafeInteger(value) && Number(value) > 0
const count = (value: unknown) => Number.isSafeInteger(value) && Number(value) >= 0
const namedIds = (value: unknown) => Array.isArray(value) && value.every(item => object(item) && id(item.id) && typeof item.name === 'string')
export function validNexusNodes(value: unknown): value is NexusNode[] {
  if (!Array.isArray(value)) return false
  const identities = new Set<string>(), ids = new Set<number>()
  return value.every(node => {
    if (!object(node) || !['id', 'legacy_node_id', 'legacy_machine_id', 'legacy_host_id', 'server_id', 'exported_at', 'imported_at'].every(key => id(node[key]))
      || !['source_id', 'server_name', 'machine_name', 'name', 'protocol', 'public_host', 'public_port', 'listen_port', 'rate'].every(key => typeof node[key] === 'string')
      || !['enabled', 'visible', 'machine_active', 'timed_rate_enabled', 'certificate_saved'].every(key => typeof node[key] === 'boolean')
      || !['pending_agent', 'prepared', 'applying', 'applied', 'paused'].includes(String(node.state)) || node.configuration_saved !== true || !Number.isSafeInteger(node.sort_order)
      || !(node.parent_id === null || id(node.parent_id)) || !Array.isArray(node.tags) || !node.tags.every(tag => typeof tag === 'string')
      || !namedIds(node.groups) || !namedIds(node.routes) || !object(node.protocol_summary)
      || !Object.values(node.protocol_summary).every(v => typeof v === 'string' || typeof v === 'boolean' || typeof v === 'number' && Number.isFinite(v))
      || !['custom_route_count', 'custom_outbound_count', 'timed_rate_count'].every(key => count(node[key]))) return false
    const key = `${node.source_id}:${node.legacy_node_id}`
    if (identities.has(key) || ids.has(Number(node.id))) return false
    identities.add(key); ids.add(Number(node.id)); return true
  })
}

export function filterNexusNodes(nodes: NexusNode[], search: string, protocol: string, server: string) {
  const needle = search.trim().toLocaleLowerCase()
  return nodes.filter(node => (!protocol || node.protocol === protocol) && (!server || String(node.server_id) === server)
    && (!needle || [node.name, node.public_host, node.server_name, node.machine_name, String(node.legacy_node_id), ...node.tags, ...node.groups.map(group => group.name)].some(value => value.toLocaleLowerCase().includes(needle))))
}

export const nexusProtocol = (node: NexusNode) => node.protocol === 'vless' && node.protocol_summary.tls === 2 ? 'VLESS · Reality'
  : node.protocol === 'hysteria' && node.protocol_summary.version === 2 ? 'Hysteria2'
  : node.protocol === 'shadowsocks' ? 'Shadowsocks 2022' : node.protocol
export const nexusEndpoint = (node: NexusNode) => `${node.public_host.includes(':') && !node.public_host.startsWith('[') ? `[${node.public_host}]` : node.public_host}:${node.public_port}`

export const nexusRuntimeState = (node: NexusNode) => ({ pending_agent: '待 Agent 接管', prepared: '配置已准备', applying: '等待设备应用', applied: 'apsp 已接管', paused: '接管已暂停' })[node.state]
