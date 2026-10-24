import type { Account, Operation, Overview, PowerJob, Resource } from './types'
import { billUsable } from './types'
import { cdtGuardExceeded } from './guardUsage'

export type CurrentCloud = () => Overview | undefined
const stale = '云服务信息正在刷新或刷新失败，请成功刷新后再提交；当前草稿已保留。'
const changed = '云账号、资源或操作已变化，请关闭草稿后重新核对；当前草稿已保留。'
export const revisionMatches = (draft: number | undefined, current: number | undefined) => Number.isSafeInteger(draft) && draft! > 0 && draft === current
export const cloudBusy = (data: Overview, id: string) => [...data.operations, ...(data.power_jobs ?? [])].some(item => item.resource_id === id && ['queued', 'running', 'uncertain'].includes(item.status))
export const cloudError = (current: CurrentCloud) => current() ? '' : stale

export function accountError(current: CurrentCloud, draft?: Account, enabled = false) {
  const data = current()
  if (!data) return stale
  if (!draft) return data.accounts.length < 8 ? '' : '最多登记 8 个云账号。'
  const account = data.accounts.find(item => item.id === draft.id)
  return !account || !revisionMatches(draft.revision, account.revision) || (enabled && !account.enabled) ? changed : ''
}

export function resourceError(current: CurrentCloud, draft: Resource, options: { managed?: boolean; idle?: boolean; power?: boolean; accountRevision?: number } = {}) {
  const data = current()
  if (!data) return stale
  const resource = data.resources.find(item => item.id === draft.id)
  const account = data.accounts.find(item => item.id === draft.account_id)
  if (!resource || !account || !revisionMatches(draft.revision, resource.revision) ||
    !revisionMatches(account.revision, account.revision) || resource.account_id !== draft.account_id ||
    resource.cloud_id !== draft.cloud_id || resource.region !== draft.region || resource.kind !== draft.kind ||
    (Object.hasOwn(options, 'accountRevision') && !revisionMatches(options.accountRevision, account.revision)) ||
    (options.managed && !account.enabled)) return changed
  if (options.power && (resource.kind !== 'ecs' || !Array.isArray(data.power_jobs))) return '当前面板未确认支持此资源的启停管理，请刷新后核对。'
  return options.idle && cloudBusy(data, draft.id) ? '此资源已有待执行或待核对操作，请先核对结果。' : ''
}

export function registrationError(current: CurrentCloud, accountId: string, accountRevision?: number) {
  const data = current()
  if (!data) return stale
  const account = data.accounts.find(item => item.id === accountId)
  return !account || !revisionMatches(accountRevision, account.revision) ? changed : data.resources.length >= 32 ? '最多登记 32 个云资源。' : ''
}

export function powerError(current: CurrentCloud, draft: Resource, action?: 'start' | 'stop', accountRevision?: number) {
  const reason = resourceError(current, draft, { managed: true, idle: true, power: true, accountRevision })
  if (reason || action !== 'start') return reason
  const data = current()!, resource = data.resources.find(item => item.id === draft.id)!, policy = resource.power_policy
  const account = data.accounts.find(item => item.id === resource.account_id)!
  if (policy?.enabled && policy.cdt_guard) return cdtGuardExceeded(account, resource) !== false
    ? 'CDT 用量保护生效，或当前用量无法核对；请先核对月度状态与守护策略。' : ''
  return policy?.enabled && policy.threshold_action === 'stop' && (resource.threshold_hold || !billUsable(account) ||
    account.bill!.usage_micro_gb! * 100 >= policy.limit_gb * 1e6 * policy.threshold_percent)
    ? '流量停机保护生效，或当前账单无法核对；请先核对用量与阈值策略。' : ''
}

export function operationError(current: CurrentCloud, draft: Operation | PowerJob, power: boolean, states: string[], confirm = false, now = Date.now() / 1000) {
  const data = current()
  if (!data) return stale
  const latest = (power ? data.power_jobs : data.operations)?.find(item => item.id === draft.id)
  const resource = data.resources.find(item => item.id === draft.resource_id)
  if (!latest || !resource || latest.resource_id !== draft.resource_id || latest.status !== draft.status || !states.includes(latest.status)) return changed
  if (!confirm) return ''
  const account = data.accounts.find(item => item.id === resource.account_id)
  if (!account?.enabled || !revisionMatches(latest.resource_revision, resource.revision) ||
    !revisionMatches(latest.account_revision, account.revision) || latest.resource_revision !== draft.resource_revision ||
    latest.account_revision !== draft.account_revision || latest.source !== 'manual' || draft.source !== 'manual' ||
    !Number.isSafeInteger(latest.expires_at) || latest.expires_at <= now || latest.expires_at !== draft.expires_at ||
    latest.before_state.cloud_id !== resource.cloud_id || latest.before_state.region !== resource.region ||
    JSON.stringify(latest.before_state) !== JSON.stringify(draft.before_state) || cloudBusy(data, resource.id)) return changed
  if (power) {
    if (!('action' in latest) || !('action' in draft) || latest.action !== draft.action || latest.stop_mode !== draft.stop_mode) return changed
  } else if (!('target' in latest) || !('target' in draft) || latest.target.bandwidth_mbps !== draft.target.bandwidth_mbps || latest.target.charge_type !== draft.target.charge_type) return changed
  return ''
}

export function credentialError(key: string, secret: string, required = false) {
  return Boolean(key.trim()) !== Boolean(secret.trim()) || (required && (!key.trim() || !secret.trim())) ? '请同时填写访问密钥 ID 与 Secret；编辑时两项留空保留。' : ''
}

export function checked<T>(error: () => string, request: () => Promise<T>) {
  const reason = error()
  if (reason) throw new Error(reason)
  return request()
}
