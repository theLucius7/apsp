import type { ExternalEntry } from './externalAccessTypes'
import { hasExactKeys, isInteger, isPort, isRecord, isUuid } from '@/lib/validate'
/** `legacy_group_id` marks a group that stands for an original Nexus permission group (ADR 0115). */
export type PolicyGroup = { id: number; name: string; node_ids: number[]; chain_ids: number[]; member_count: number; external_nodes?: ExternalEntry[]; legacy_group_id?: number | null; migrated_node_ids?: number[]; legacy_lines?: number }
export type PackageGroup = { id: number; name: string; monthly_bytes: string | null; reset_day: number; reset_hour: number; reset_minute: number; timezone: string; duration_days: number }
export type ProxyResourceKind = 'direct' | 'chain'
export type ProxyResourceFilter = 'all' | 'direct' | 'chains'
export type ProxyResourceServerRole = 'any' | 'entry' | 'middle' | 'exit'
export type ResourceEndpoint = {
  id: number; name: string; server_id: number; server_name: string; protocol: string; port: number; public_port: number;
  public_host: string; sni: string; enabled: boolean; node_deleted: boolean; server_deleted: boolean;
  plugin_enabled: boolean; online: boolean; desired_revision: number | null; applied_revision: number | null;
  applied_observed_at: number | null;
}
export type ProxyResource = {
  kind: ProxyResourceKind; id: number; name: string; entry: ResourceEndpoint; exit: ResourceEndpoint | null;
  available: boolean; unavailable_reasons: string[]; policy_group_ids: number[]; user_count: number;
  chain_refs: { id: number; name: string; role: 'entry' | 'exit'; hop_position: number | null; generation: number; state: 'desired' | 'applied' | 'candidate' | 'recovery' | 'retained' | 'unresolved' }[];
  settings_revision: number; path_kind: 'ordered' | null; hops: PublicHop[]; path_state: PathState | null;
}
export type PublicHop = { kind: 'managed'; position: number; node_id: number; endpoint_version_id: string; endpoint: ResourceEndpoint }
  | { kind: 'subscription'; position: number; source_id: number; source_name: string; identity_epoch: number;
    external_node_id: string; node_version_id: string; source_revision_id: string; update_mode: 'follow_node' | 'pinned';
    name: string; protocol: string; server: string; server_port: number; sni: string | null; transport: string | null;
    capabilities: { tcp: boolean; udp: boolean }; source_archived: boolean; node_present: boolean; update_error: string | null }
/** A chain's hops as one line: managed hops by node and server, external hops by source and pinned or followed version. */
export function publicPathText(hops: PublicHop[]) { return hops.map(hop => hop.kind === 'managed' ? `受管 ${hop.endpoint.name}（服务器 #${hop.endpoint.server_id}）` : `订阅 ${hop.source_name} / ${hop.name}（${hop.update_mode === 'pinned' ? '固定' : '跟随'} ${hop.node_version_id.slice(0, 8)}）`).join(' → ') }
export type PathDependency = { server_id: number; role: 'entry' | 'managed'; hop_position: number | null; generation: number; stage: string;
  required_revision: number | null; applied_revision: number | null; bundle_sha256: string | null; state: 'pending' | 'ready' | 'failed' | 'retired'; observed_at: number | null }
export type PathProbe = { stage: 'candidate' | 'switched'; request_id: string; state: 'pending' | 'verified' | 'failed' | 'expired'; observed_at: number | null; error: string | null }
export type PathPhase = 'preparing_dependencies' | 'preparing_entry' | 'probing_candidate' | 'switching_entry' | 'probing_switched' | 'fixing_barrier' | 'retiring_old' | 'applied' | 'restoring' | 'failed' | 'retiring' | 'retired'
export type PathState = { desired_generation: number; candidate_generation: number | null; applied_generation: number | null;
  recovery_generation: number | null; minimum_generation: number; phase: PathPhase; capabilities: { tcp: boolean; udp: boolean };
  last_error: string | null; dependencies: PathDependency[]; probe: PathProbe | null;
  generations: { generation: number; state: 'desired' | 'applied' | 'candidate' | 'recovery'; hops: PublicHop[] }[] }

const nullableInteger = (value: unknown) => value === null || isInteger(value)
const nullableText = (value: unknown) => value === null || typeof value === 'string'
const capabilities = (value: unknown) => isRecord(value) && hasExactKeys(value, 'tcp udp') && typeof value.tcp === 'boolean' && typeof value.udp === 'boolean'
export function validPublicHop(value: unknown): value is PublicHop {
  if (!isRecord(value) || !isInteger(value.position, 1) || value.position > 8) return false
  if (value.kind === 'managed') return hasExactKeys(value, 'kind position node_id endpoint_version_id endpoint') && isInteger(value.node_id, 1) && isUuid(value.endpoint_version_id) && validResourceEndpoint(value.endpoint) && value.node_id === value.endpoint.id
  return value.kind === 'subscription' && hasExactKeys(value, 'kind position source_id source_name identity_epoch external_node_id node_version_id source_revision_id update_mode name protocol server server_port sni transport capabilities source_archived node_present update_error')
    && isInteger(value.source_id, 1) && isInteger(value.identity_epoch, 1) && typeof value.source_name === 'string' && isUuid(value.external_node_id) && isUuid(value.node_version_id) && isUuid(value.source_revision_id)
    && ['follow_node', 'pinned'].includes(String(value.update_mode)) && typeof value.name === 'string' && typeof value.protocol === 'string' && value.protocol.length > 0 && typeof value.server === 'string' && value.server.length > 0 && !/[\s/@?#\\]/.test(value.server)
    && ['sni', 'transport', 'update_error'].every(key => nullableText(value[key])) && isPort(value.server_port) && capabilities(value.capabilities) && typeof value.source_archived === 'boolean' && typeof value.node_present === 'boolean'
}
export function validPublicHops(value: unknown): value is PublicHop[] { return Array.isArray(value) && value.length >= 1 && value.length <= 8 && value.every((hop, index) => validPublicHop(hop) && hop.position === index + 1) }
export function validPathState(value: unknown): value is PathState {
  if (!isRecord(value)) return false
  const generations = value.generations
  return hasExactKeys(value, 'desired_generation candidate_generation applied_generation recovery_generation minimum_generation phase capabilities last_error dependencies probe generations')
    && isInteger(value.desired_generation, 1) && ['candidate_generation', 'applied_generation', 'recovery_generation'].every(key => value[key] === null || isInteger(value[key], 1) && value[key] <= Number(value.desired_generation)) && isInteger(value.minimum_generation) && value.minimum_generation <= value.desired_generation
    && ['preparing_dependencies', 'preparing_entry', 'probing_candidate', 'switching_entry', 'probing_switched', 'fixing_barrier', 'retiring_old', 'applied', 'restoring', 'failed', 'retiring', 'retired'].includes(String(value.phase))
    && capabilities(value.capabilities) && nullableText(value.last_error) && Array.isArray(value.dependencies) && value.dependencies.length <= 512
    && value.dependencies.every(dep => isRecord(dep) && hasExactKeys(dep, 'server_id role hop_position generation stage required_revision applied_revision bundle_sha256 state observed_at')
      && isInteger(dep.server_id, 1) && ['entry', 'managed'].includes(String(dep.role)) && (dep.role === 'entry' ? dep.hop_position === null : isInteger(dep.hop_position, 1) && dep.hop_position <= 8)
      && isInteger(dep.generation, 1) && typeof dep.stage === 'string' && ['required_revision', 'applied_revision', 'observed_at'].every(key => nullableInteger(dep[key]))
      && (dep.bundle_sha256 === null || typeof dep.bundle_sha256 === 'string' && /^[a-f0-9]{64}$/.test(dep.bundle_sha256)) && ['pending', 'ready', 'failed', 'retired'].includes(String(dep.state)))
    && (value.probe === null || isRecord(value.probe) && hasExactKeys(value.probe, 'stage request_id state observed_at error') && ['candidate', 'switched'].includes(String(value.probe.stage)) && isUuid(value.probe.request_id)
      && ['pending', 'verified', 'failed', 'expired'].includes(String(value.probe.state)) && nullableInteger(value.probe.observed_at) && (value.probe.state !== 'verified' || isInteger(value.probe.observed_at)) && nullableText(value.probe.error))
    && Array.isArray(generations) && generations.length >= 1 && generations.length <= 4 && generations.every(view => isRecord(view) && hasExactKeys(view, 'generation state hops')
      && isInteger(view.generation, 1) && ['desired', 'applied', 'candidate', 'recovery'].includes(String(view.state)) && validPublicHops(view.hops)
      && view.generation === value[`${view.state}_generation`]) && new Set(generations.map(view => view.state)).size === generations.length
    && generations.some(view => view.state === 'desired') && ['applied', 'candidate', 'recovery'].every(state => value[`${state}_generation`] === null || generations.some(view => view.state === state))
}

export function proxyResourceKey(resource: Pick<ProxyResource, 'kind' | 'id'>) { return `${resource.kind}:${resource.id}` }
export function validResourceEndpoint(value: unknown): value is ResourceEndpoint {
  return isRecord(value) && hasExactKeys(value, 'id name server_id server_name protocol port public_port public_host sni enabled node_deleted server_deleted plugin_enabled online desired_revision applied_revision applied_observed_at') && isInteger(value.id, 1) && isInteger(value.server_id, 1)
    && ['name', 'server_name', 'protocol', 'public_host', 'sni'].every(key => typeof value[key] === 'string')
    && isPort(value.port) && isPort(value.public_port)
    && ['enabled', 'node_deleted', 'server_deleted', 'plugin_enabled', 'online'].every(key => typeof value[key] === 'boolean')
    && ['desired_revision', 'applied_revision', 'applied_observed_at'].every(key => nullableInteger(value[key]))
}
export function validProxyResource(value: unknown): value is ProxyResource {
  return isRecord(value) && hasExactKeys(value, 'kind id name entry exit available unavailable_reasons policy_group_ids user_count chain_refs settings_revision path_kind hops path_state') && (value.kind === 'direct' || value.kind === 'chain') && isInteger(value.id, 1)
    && typeof value.name === 'string' && validResourceEndpoint(value.entry)
    && (value.exit === null || validResourceEndpoint(value.exit)) && isInteger(value.settings_revision, 1)
    && (value.kind === 'direct' ? value.exit === null && value.entry.id === value.id && value.path_kind === null && Array.isArray(value.hops) && value.hops.length === 0 && value.path_state === null
      : value.path_kind === 'ordered' && validPublicHops(value.hops) && validPathState(value.path_state)
        && (value.hops[value.hops.length - 1].kind === 'managed' ? validResourceEndpoint(value.exit) && value.exit.id === (value.hops[value.hops.length - 1] as Extract<PublicHop, { kind: 'managed' }>).node_id : value.exit === null)
        && JSON.stringify(value.path_state.generations.find(view => view.state === 'desired')?.hops) === JSON.stringify(value.hops))
    && typeof value.available === 'boolean' && Array.isArray(value.unavailable_reasons) && value.unavailable_reasons.every(reason => typeof reason === 'string')
    && Array.isArray(value.policy_group_ids) && value.policy_group_ids.every(id => isInteger(id, 1)) && isInteger(value.user_count)
    && Array.isArray(value.chain_refs) && value.chain_refs.every(ref => isRecord(ref) && hasExactKeys(ref, 'id name role hop_position generation state') && isInteger(ref.id, 1) && typeof ref.name === 'string' && ['entry', 'exit'].includes(String(ref.role))
      && (ref.role === 'entry' ? ref.hop_position === null : isInteger(ref.hop_position, 1) && ref.hop_position <= 8) && isInteger(ref.generation, 1) && ['desired', 'applied', 'candidate', 'recovery', 'retained', 'unresolved'].includes(String(ref.state)))
}
export function validProxyResources(value: unknown): value is ProxyResource[] {
  return Array.isArray(value) && value.every(validProxyResource) && new Set(value.map(proxyResourceKey)).size === value.length
}
export function filterProxyResources(resources: ProxyResource[], kind: ProxyResourceFilter, serverId?: number, role: ProxyResourceServerRole = 'any') {
  return resources.filter(resource => (kind === 'all' || resource.kind === (kind === 'chains' ? 'chain' : 'direct'))
    && (serverId === undefined || resource.kind === 'direct' && resource.entry.server_id === serverId || (role === 'any' || role === 'entry') && resource.entry.server_id === serverId
      || resource.hops.some(hop => hop.kind === 'managed' && hop.endpoint.server_id === serverId && (role === 'any' || role === 'exit' && hop.position === resource.hops.length || role === 'middle' && hop.position < resource.hops.length))))
}
export function proxyResourceCounts(resources: ProxyResource[]) {
  return { total: resources.length, direct: resources.filter(resource => resource.kind === 'direct').length,
    chains: resources.filter(resource => resource.kind === 'chain').length,
    endpoints: new Set(resources.flatMap(resource => [resource.entry, ...resource.hops.flatMap(hop => hop.kind === 'managed' ? [hop.endpoint] : [])]).filter(endpoint => !endpoint.node_deleted && !endpoint.server_deleted).map(endpoint => endpoint.id)).size }
}
export type UserPolicies = { group_ids: number[] }
export type Entitlement = {
  user_id: number; package_group_id: number | null; package_name: string | null; monthly_bytes: string | null;
  reset_day: number | null; reset_hour: number | null; reset_minute: number | null; timezone: string | null;
  starts_at: number | null; expires_at: number | null; cycle_start: number | null; next_reset: number | null;
  used_bytes: string; status: 'paused' | 'unmetered' | 'not_started' | 'active' | 'expired' | 'exhausted'; allowed: boolean;
}
export const statusText = { paused: '已暂停', unmetered: '未分配套餐', not_started: '尚未生效', active: '使用中', expired: '已到期', exhausted: '本期流量已用完' }
export const scheduleText = (p: Pick<PackageGroup, 'reset_day' | 'reset_hour' | 'reset_minute' | 'timezone'>) => `每月 ${p.reset_day} 日 ${String(p.reset_hour).padStart(2, '0')}:${String(p.reset_minute).padStart(2, '0')}（${p.timezone}）`

export function quotaBytes(amount: string, unit: string): string | null {
  if (!amount.trim()) return null
  if (!/^[1-9][0-9]*$/.test(amount) || !['B', 'GiB'].includes(unit)) throw new Error('每月流量需为正整数，留空表示不限量。')
  const value = BigInt(amount) * (unit === 'GiB' ? 1073741824n : 1n)
  if (value > 18446744073709551615n) throw new Error('每月流量超出支持范围。')
  return value.toString()
}
