// Feature tiers from docs/product-map.md that change how a feature appears.
// Experimental (降级) features stay usable and carry a label; frozen (冻结)
// features only receive security fixes and stay hidden unless 面板设置 shows
// them. Core, supporting and optional features need no marker.
export type Tier = 'experimental' | 'frozen'

export const tierLabels: Record<Tier, string> = { experimental: '实验', frozen: '冻结' }

export const tierDescriptions: Record<Tier, string> = {
  experimental: '实验功能：保留可用，只修缺陷和安全问题，不再扩展；不属于代理网络主线。',
  frozen: '冻结功能：不再投入，只修安全问题；默认隐藏，可在「面板设置」中显示。',
}

/** Drops frozen entries unless the panel shows frozen features. */
export function visibleByTier<T extends { tier?: Tier }>(entries: readonly T[], showFrozen: boolean): T[] {
  return entries.filter(entry => entry.tier !== 'frozen' || showFrozen)
}
