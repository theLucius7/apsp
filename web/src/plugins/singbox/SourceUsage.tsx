import { bytes } from '../../format'
import { dateText } from './groupTypes'
import type { SourceTraffic } from './orderedSourceTypes'

// The provider's own report (subscription-userinfo); the panel does not meter external nodes.
export function SourceUsage({ traffic }: { traffic: SourceTraffic }) {
  if (traffic.upload === undefined && traffic.download === undefined && traffic.total === undefined && traffic.expire === undefined) return null
  const used = traffic.upload !== undefined && traffic.download !== undefined && Number.isSafeInteger(traffic.upload + traffic.download) ? traffic.upload + traffic.download : undefined
  return <div className="text-muted-foreground my-2.5 flex flex-wrap gap-3 text-xs"><span>来源用量：{used !== undefined ? bytes(used) : '未知'} / {traffic.total !== undefined ? bytes(traffic.total) : '未知额度'}</span>{traffic.expire !== undefined && <span>到期：{dateText(traffic.expire)}</span>}<small>提供方上报 · {dateText(traffic.updated_at ?? null)}</small></div>
}
