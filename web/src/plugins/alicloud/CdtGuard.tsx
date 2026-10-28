import { Field, ErrorNotice } from '../../components'
import { Checkbox } from '@/components/ui/checkbox'
import { Input } from '@/components/ui/input'
import { message, time } from './types'
import type { CdtGuardConfig, Resource } from './types'

export const GIB = 1024 ** 3
export const defaultCdtGuard: CdtGuardConfig = { threshold_bytes: 150 * GIB, jump_bytes: 20 * GIB, auto_start: true }
const gib = (bytes: number) => (bytes / GIB).toLocaleString('zh-CN', { maximumFractionDigits: 2 })

export function CdtGuardFields({ value, onChange }: { value: CdtGuardConfig; onChange: (value: CdtGuardConfig) => void }) {
  return <>
    <div className="grid gap-x-4 sm:grid-cols-2">
      <Field label="CDT 停机线（GiB）"><Input required type="number" min={0.001} max={1000000} step="any" value={value.threshold_bytes / GIB} onChange={e => onChange({ ...value, threshold_bytes: Math.round(Number(e.target.value) * GIB) })} /></Field>
      <Field label="突增检测阈值（GiB）"><Input required type="number" min={0.001} max={1000000} step="any" value={value.jump_bytes / GIB} onChange={e => onChange({ ...value, jump_bytes: Math.round(Number(e.target.value) * GIB) })} /></Field>
    </div>
    <p className="text-muted-foreground text-xs leading-relaxed">首次零用量无法证明已开通 CDT，需在云控制台核对。曾查询到有效非零用量后，可接受完整零读数进行月度重置；空响应或缺少目标额度池仍暂停操作。</p>
    <label className="my-4 flex items-start gap-2.5 text-sm leading-relaxed"><Checkbox checked={value.auto_start} onCheckedChange={checked => onChange({ ...value, auto_start: checked === true })} className="mt-0.5" />用量低于停机线时自动开机</label>
    <p className="text-muted-foreground mt-3 text-xs leading-relaxed">每分钟读取账号所属中国内地或海外额度池的全部 BGP 用量，包含不同地域及 IPv4、IPv6 产品。按北京时间自然月保留最高用量；单次突增达到阈值后，本月最大突增作为延迟上报缓冲，叠加到用量后判断停机线。</p>
    <p className="text-muted-foreground mt-3 text-xs leading-relaxed">达到停机线时正常关机并请求节省停机；低于停机线时可自动启动普通或抢占式实例。手动停机暂停自动开机。跨月等待用量重置，异常或不完整数据保持实例原状。CDT 接口没有可信账期时间戳，缓冲无法保证零超额费用。</p>
    <p className="text-muted-foreground mt-3 text-xs leading-relaxed">迁移已有脚本时，请先用迁移工具导入本月最高用量与最大突增，并停用原定时任务，再启用本策略。</p>
  </>
}

export function CdtGuardDetails({ resource }: { resource: Resource }) {
  const config = resource.power_policy?.cdt_guard, state = resource.cdt_guard_state
  if (!config) return null
  const backlog = state && state.max_jump_bytes >= config.jump_bytes ? state.max_jump_bytes : 0
  return <section aria-label="CDT 脚本守护">
    <strong>CDT 脚本守护 · {resource.power_policy?.enabled ? '已启用' : '已暂停'}</strong>
    <p className="text-sm leading-relaxed">停机线 {gib(config.threshold_bytes)} GiB · 突增阈值 {gib(config.jump_bytes)} GiB · {config.auto_start ? '低用量自动开机' : '仅自动停机'}</p>
    {state ? <><p className="text-sm leading-relaxed">{state.billing_cycle} · 月度最高 {gib(state.max_usage_bytes)} GiB · 最大突增 {gib(state.max_jump_bytes)} GiB · 当前缓冲 {gib(backlog)} GiB</p><p className="text-muted-foreground mt-3 text-xs leading-relaxed">最近用量查询：{time(state.queried_at || null)}{!state.queried_at ? ' · 已导入历史，等待首次查询' : ''} · 超过 2 分钟或跨月的读数不用于启停</p></> : <p>尚未建立月度状态。</p>}
    <ErrorNotice message={message(resource.cdt_guard_error)} />
  </section>
}
