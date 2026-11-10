import { Badge, Field, Meter } from '../../components'
import { Checkbox } from '@/components/ui/checkbox'
import { Input } from '@/components/ui/input'
import { gib, time } from '../../resources/cloud/types'
import { cdtGuardProgress } from './guardUsage'
import type { CdtGuardConfig, Resource } from '../../resources/cloud/types'

export const GIB = 1024 ** 3
export const defaultCdtGuard: CdtGuardConfig = { threshold_bytes: 150 * GIB, jump_bytes: 20 * GIB, auto_start: true }

export function CdtGuardFields({ value, onChange }: { value: CdtGuardConfig; onChange: (value: CdtGuardConfig) => void }) {
  return <>
    <div className="grid gap-x-4 sm:grid-cols-2">
      <Field label="CDT 停机线（GiB）"><Input required type="number" min={0.001} max={1000000} step="any" value={value.threshold_bytes / GIB} onChange={e => onChange({ ...value, threshold_bytes: Math.round(Number(e.target.value) * GIB) })} /></Field>
      <Field label="突增检测阈值（GiB）"><Input required type="number" min={0.001} max={1000000} step="any" value={value.jump_bytes / GIB} onChange={e => onChange({ ...value, jump_bytes: Math.round(Number(e.target.value) * GIB) })} /></Field>
    </div>
    <p className="text-muted-foreground text-xs leading-relaxed">首次零用量不能证明已开通 CDT，请在控制台核对；空响应或缺少额度池时暂停操作。</p>
    <label className="my-4 flex items-start gap-2.5 text-sm leading-relaxed"><Checkbox checked={value.auto_start} onCheckedChange={checked => onChange({ ...value, auto_start: checked === true })} className="mt-0.5" />用量低于停机线时自动开机</label>
    <p className="text-muted-foreground mt-3 text-xs leading-relaxed">每分钟读取额度池的全部 BGP 用量，按北京时间自然月取最高值；单次突增达到阈值后，本月最大突增作为缓冲叠加到用量上，再与停机线比较。</p>
    <p className="text-muted-foreground mt-3 text-xs leading-relaxed">达到停机线时正常关机并请求节省停机，低于停机线时可自动开机；手动停机会暂停自动开机，数据异常时不动实例。缓冲不能保证零超额费用。</p>
    <p className="text-muted-foreground mt-3 text-xs leading-relaxed">从旧脚本迁移时，先导入本月最高用量与最大突增并停用原定时任务，再启用本策略。</p>
  </>
}

/** One line of the resource row: how far the month's CDT usage is from the guard's stop line. */
export function CdtGuardSummary({ resource }: { resource: Resource }) {
  const config = resource.power_policy?.cdt_guard
  if (!config) return null
  const progress = cdtGuardProgress(resource), paused = !resource.power_policy?.enabled
  return <section aria-label="CDT 脚本守护" data-slot="cdt-guard" className="grid gap-1">
    <div className="flex flex-wrap items-center gap-x-2 gap-y-1 text-[13px]">
      <span className="font-medium">CDT 守护</span>
      {paused && <Badge>已暂停</Badge>}
      {progress ? <span className={progress.left === 0 ? 'text-destructive' : undefined}>{progress.left === 0 ? '已达停机线' : `距停机线 ${gib(progress.left)} GiB`}</span> : <span className="text-muted-foreground">尚未建立月度状态</span>}
    </div>
    {progress && <Meter value={progress.percent} label={`CDT 用量占停机线 ${Math.round(progress.percent)}%`} className="max-w-72" />}
    <p className="text-muted-foreground text-xs leading-relaxed">停机线 {gib(config.threshold_bytes)} GiB{config.auto_start ? ' · 低于停机线自动开机' : ' · 仅自动停机'}{progress && progress.backlog > 0 ? ` · 含突增缓冲 ${gib(progress.backlog)} GiB` : ''}{progress?.stale && !paused ? ' · 读数超过 2 分钟，暂不据此启停' : ''}</p>
  </section>
}

/** The guard's settings and month state, folded into the resource details. */
export function cdtGuardFacts(resource: Resource): [string, string][] {
  const progress = cdtGuardProgress(resource), config = resource.power_policy?.cdt_guard
  if (!config) return []
  return [
    ['CDT 守护', `停机线 ${gib(config.threshold_bytes)} GiB · 突增阈值 ${gib(config.jump_bytes)} GiB`],
    ...(progress ? [
      [`${progress.state.billing_cycle} 用量`, `月度最高 ${gib(progress.state.max_usage_bytes)} GiB · 最大突增 ${gib(progress.state.max_jump_bytes)} GiB · 当前缓冲 ${gib(progress.backlog)} GiB`],
      ['用量查询', progress.state.queried_at ? time(progress.state.queried_at) : '已导入历史，等待首次查询'],
    ] as [string, string][] : []),
  ]
}
