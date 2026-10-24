import type { Account, Resource } from './types'

export function cdtGuardExceeded(account: Account, resource: Resource, now = Date.now() / 1000): boolean | null {
  const config = resource.power_policy?.cdt_guard, state = resource.cdt_guard_state
  if (!config || !state || !account.enabled || resource.cdt_guard_error || state.account_revision !== account.revision ||
    state.region !== resource.region || state.cloud_id !== resource.cloud_id ||
    state.billing_cycle !== new Date((now + 28800) * 1000).toISOString().slice(0, 7) ||
    !Number.isSafeInteger(state.queried_at) || state.queried_at <= 0 || state.queried_at > now || now - state.queried_at > 120 ||
    ![state.max_usage_bytes, state.max_jump_bytes, config.threshold_bytes, config.jump_bytes].every(v => Number.isSafeInteger(v) && v >= 0) ||
    config.threshold_bytes <= 0 || config.jump_bytes <= 0 || state.max_jump_bytes > state.max_usage_bytes) return null
  const backlog = state.max_jump_bytes >= config.jump_bytes ? state.max_jump_bytes : 0
  return BigInt(state.max_usage_bytes) + BigInt(backlog) >= BigInt(config.threshold_bytes)
}

