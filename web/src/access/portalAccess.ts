import type { PasskeyInfo } from '../governance/passkeys'

export type PortalAccess = { configuration: PasskeyInfo; keys: number; url: string | null; activation_expires_at: number | null }
export function validPortalAccess(value: unknown): value is PortalAccess {
  if (!value || typeof value !== 'object') return false
  const access = value as Partial<PortalAccess>, configuration = access.configuration
  if (!configuration) return false
  return Number.isSafeInteger(access.keys) && access.keys! >= 0
    && typeof configuration.enabled === 'boolean' && typeof configuration.origin === 'string'
    && (configuration.reason === null || typeof configuration.reason === 'string')
    && (access.url === null || typeof access.url === 'string')
    && (access.activation_expires_at === null || Number.isSafeInteger(access.activation_expires_at) && access.activation_expires_at! >= 0)
}
export function portalAccessError(current: unknown, draft?: PortalAccess): string {
  if (!validPortalAccess(current)) return '用户入口状态尚未确认，请成功刷新后再操作；当前草稿已保留。'
  if (current.configuration.enabled !== true) return current.configuration.reason || '当前访问地址未启用 Passkey；当前草稿已保留。'
  if (draft && (current.keys !== draft.keys || current.configuration.origin !== draft.configuration.origin || current.url !== draft.url || current.activation_expires_at !== draft.activation_expires_at)) return '用户入口或 Passkey 状态已变化，请重新打开并核对重置操作；当前草稿已保留。'
  return ''
}
