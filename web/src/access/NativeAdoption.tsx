import { useEffect, useRef, useState } from 'react'
import { api, ApiError, errorMessage } from '../api'
import { Badge, CopyButton, DialogPresence, ErrorNotice, Loading, Modal } from '../components'
import { bytes, dateText } from '../format'
import { statusText } from './groupTypes'
import { Button } from '@/components/ui/button'
import { Checkbox } from '@/components/ui/checkbox'
import { Textarea } from '@/components/ui/textarea'
import { Notice } from '@/components/panel'
import { isInteger, isRecord } from '@/lib/validate'

type Format = 'singbox' | 'clash' | 'links'
type Comparison = Format | 'blocked'
type PreviewEntitlement = {
  user_id: number; status: string; allowed: boolean
  package_group_id?: number | null; package_name?: string | null
  monthly_bytes?: string | null; used_bytes: string
  starts_at?: number | null; expires_at?: number | null
  cycle_start?: number | null; next_reset?: number | null
  timezone?: string | null
}
type Preview = {
  digest: string; adopted: boolean; blocked?: boolean; comparison_modes?: string[]
  blockers: string[]; old_subscription_url: string; route_confirmation_required: boolean
  formats: Partial<Record<Format, string>>; unavailable_formats: Partial<Record<Format, string>>
  entitlement?: PreviewEntitlement
  granted_nodes: number; managed_nodes: number; external_nodes: number
}
const formats: readonly Format[] = ['singbox', 'clash', 'links']
const names: Record<Format, string> = { singbox: 'sing-box', clash: 'Clash Meta', links: '分享链接' }
const nullableTimestamp = (value: unknown): value is number | null | undefined =>
  value === undefined || value === null || typeof value === 'number' && Number.isSafeInteger(value)

function validEntitlement(value: unknown): value is PreviewEntitlement {
  return isRecord(value) && isInteger(value.user_id, 1) && typeof value.status === 'string' && typeof value.allowed === 'boolean'
    && (value.package_group_id === undefined || value.package_group_id === null || isInteger(value.package_group_id, 1))
    && (value.package_name === undefined || value.package_name === null || typeof value.package_name === 'string')
    && (value.monthly_bytes === undefined || value.monthly_bytes === null || typeof value.monthly_bytes === 'string' && /^\d+$/.test(value.monthly_bytes))
    && typeof value.used_bytes === 'string' && /^\d+$/.test(value.used_bytes)
    && [value.starts_at, value.expires_at, value.cycle_start, value.next_reset].every(nullableTimestamp)
    && (value.timezone === undefined || value.timezone === null || typeof value.timezone === 'string')
}

function validPreview(value: unknown): value is Preview {
  return isRecord(value) && typeof value.digest === 'string' && /^[a-f0-9]{64}$/.test(value.digest)
    && typeof value.adopted === 'boolean' && (value.blocked === undefined || typeof value.blocked === 'boolean')
    && Array.isArray(value.blockers) && value.blockers.every(item => typeof item === 'string')
    && typeof value.old_subscription_url === 'string' && value.route_confirmation_required === true
    && isRecord(value.formats) && Object.entries(value.formats).every(([key, content]) => formats.includes(key as Format) && typeof content === 'string')
    && isRecord(value.unavailable_formats) && Object.entries(value.unavailable_formats).every(([key, reason]) => formats.includes(key as Format) && typeof reason === 'string')
    && (value.comparison_modes === undefined || Array.isArray(value.comparison_modes) && value.comparison_modes.every(mode => typeof mode === 'string'))
    && (value.entitlement === undefined || validEntitlement(value.entitlement))
    && [value.granted_nodes, value.managed_nodes, value.external_nodes].every(count => isInteger(count))
}

function entitlementSummary(entitlement: PreviewEntitlement): string {
  const status = (statusText as Record<string, string>)[entitlement.status] ?? entitlement.status
  const pkg = entitlement.package_name ? `套餐「${entitlement.package_name}」（${status}）` : status
  const usage = `本期已用 ${bytes(entitlement.used_bytes)}${entitlement.monthly_bytes ? ` / ${bytes(entitlement.monthly_bytes)}` : '（不限量）'}`
  const reset = entitlement.next_reset ? ` · 下次重置 ${dateText(entitlement.next_reset, entitlement.timezone)}` : ''
  const expiry = entitlement.expires_at ? ` · 到期 ${dateText(entitlement.expires_at, entitlement.timezone)}` : ''
  return `${pkg} · ${usage}${reset}${expiry}`
}

/** Mounted only for a known migration user; secrets are read after opening the dialog. */
export default function NativeAdoption({ userId, problem, onApplied }: { userId: number; problem: () => string; onApplied: () => void }) {
  const [session, setSession] = useState(0)
  const [opened, setOpened] = useState(false)
  return <section className="my-4 border-t pt-4" aria-label="原生订阅接管">
    <h3 className="text-sm font-semibold">转由 apsp 生成订阅</h3>
    <p className="text-muted-foreground my-2 text-xs leading-relaxed">先比较原订阅与 apsp 候选配置，确认旧地址已指向 apsp，再采用当前用户。预览需要管理员再次验证。</p>
    <Button variant="outline" disabled={Boolean(problem())} onClick={() => { if (!problem()) { setSession(value => value + 1); setOpened(true) } }}>比较并接管订阅</Button>
    <DialogPresence>{opened && <AdoptionReview key={`${userId}:${session}`} userId={userId} problem={problem} onClose={() => setOpened(false)} onApplied={() => { setOpened(false); onApplied() }} />}</DialogPresence>
  </section>
}

function AdoptionReview({ userId, problem, onClose, onApplied }: { userId: number; problem: () => string; onClose: () => void; onApplied: () => void }) {
  const [preview, setPreview] = useState<Preview>()
  const [loading, setLoading] = useState(true)
  const [applying, setApplying] = useState(false)
  const [error, setError] = useState('')
  const [reload, setReload] = useState(0)
  const [selected, setSelected] = useState<Format>()
  const [viewed, setViewed] = useState<Format[]>([])
  const [compared, setCompared] = useState<Comparison[]>([])
  const [routeReady, setRouteReady] = useState(false)
  const [confirmed, setConfirmed] = useState(false)
  const [addressVisible, setAddressVisible] = useState(false)
  const alive = useRef(true)
  const requestRef = useRef<AbortController | null>(null)
  const clearSensitive = () => {
    requestRef.current?.abort()
    requestRef.current = null
    setPreview(undefined)
    setSelected(undefined)
    setViewed([])
    setCompared([])
    setRouteReady(false)
    setConfirmed(false)
    setAddressVisible(false)
  }
  const close = () => {
    if (applying) return
    clearSensitive()
    setError('')
    setLoading(false)
    onClose()
  }
  useEffect(() => {
    alive.current = true
    const controller = new AbortController()
    requestRef.current = controller
    setLoading(true); setPreview(undefined); setError(''); setSelected(undefined); setViewed([])
    setCompared([]); setRouteReady(false); setConfirmed(false); setAddressVisible(false)
    void api<unknown>(`/api/access/users/${userId}/native-adoption/preview`, 'POST', undefined, controller.signal)
      .then(value => {
        if (!alive.current || controller.signal.aborted) return
        if (!validPreview(value) || (value.entitlement && value.entitlement.user_id !== userId)) {
          throw new Error('接管预览数据不完整，请重新预览。')
        }
        setPreview(value)
      })
      .catch(reason => { if (alive.current && !controller.signal.aborted) setError(errorMessage(reason)) })
      .finally(() => { if (alive.current && !controller.signal.aborted) setLoading(false) })
    return () => { alive.current = false; controller.abort(); requestRef.current = null }
  }, [userId, reload])
  const available = formats.filter(format => typeof preview?.formats[format] === 'string')
  const blocked = preview?.blocked === true && preview.comparison_modes?.includes('blocked') === true
  const eligible = preview && !preview.adopted && preview.blockers.length === 0 && (blocked || available.length > 0)
  const selectedComparisons = compared.filter(format => format === 'blocked' ? blocked : available.includes(format))
  const busy = loading || applying
  const writeProblem = problem()
  const canApply = Boolean(eligible && selectedComparisons.length && routeReady && confirmed && !writeProblem && !busy)
  const changeCompared = (format: Comparison, checked: boolean) => {
    setConfirmed(false)
    setCompared(previous => checked ? [...previous.filter(value => value !== format), format] : previous.filter(value => value !== format))
  }
  const apply = async () => {
    if (!canApply || !preview || problem()) return
    const reviewed = preview
    setApplying(true); setError('')
    try {
      const result = await api<{ user_id: number; adopted: boolean }>(`/api/access/users/${userId}/native-adoption/adopt`, 'POST', {
        expected_digest: reviewed.digest, compared_formats: selectedComparisons,
        comparison_confirmed: confirmed, old_subscription_route_ready: routeReady,
      })
      if (!alive.current) return
      if (result.user_id !== userId || result.adopted !== true) throw new Error('尚未确认接管结果，请重新预览核对。')
      clearSensitive()
      onApplied()
    } catch (reason) {
      if (!alive.current) return
      setError(errorMessage(reason))
      setConfirmed(false)
      if (reason instanceof ApiError && reason.status === 409) {
        clearSensitive()
      }
    } finally { if (alive.current) setApplying(false) }
  }
  return <Modal title="比较并接管原生订阅" onClose={close} busy={applying} wide>
    <div data-slot="modal-body" className="space-y-4 p-4">
      <ErrorNotice message={error || writeProblem} retry={busy ? undefined : () => setReload(value => value + 1)} />
      <Notice>订阅地址和候选内容包含连接凭据，仅用于当前用户的比较。关闭对话框后清除本次显示；不会自动下载或发送到外部服务。</Notice>
      {loading ? <Loading /> : preview && <>
        <p className="text-sm">用户 #{userId} · 已授权 {preview.granted_nodes} 个节点 · 候选受管 {preview.managed_nodes} 个、外部 {preview.external_nodes} 个。</p>
        {preview.entitlement && <p className="text-muted-foreground text-xs leading-relaxed">{entitlementSummary(preview.entitlement)}</p>}
        {preview.adopted && <Notice tone="success">此用户已由 apsp 原生提供订阅，请刷新用户状态。</Notice>}
        {preview.blockers.length > 0 && <Notice tone="warning"><div><strong>当前不能接管</strong><ul className="mt-2 list-disc pl-5">{preview.blockers.map((reason, index) => <li key={index}>{reason}</li>)}</ul></div></Notice>}
        <section aria-label="原订阅地址" className="space-y-2">
          <h3 className="text-sm font-semibold">保留原订阅地址</h3>
          <p className="text-muted-foreground text-xs leading-relaxed">在原入口配置反向代理，让用户原地址继续到达 apsp；本操作不会修改 DNS 或外部代理配置。</p>
          {addressVisible ? <><code aria-label="原订阅完整地址" className="bg-muted/50 block max-h-28 overflow-auto rounded-md border p-3 text-xs whitespace-pre-wrap wrap-anywhere">{preview.old_subscription_url}</code><CopyButton text={preview.old_subscription_url} label="复制原订阅地址" /></> : <Button size="sm" variant="outline" onClick={() => setAddressVisible(true)}>显示原订阅地址</Button>}
        </section>
        <section aria-label="三种客户端格式比较" className="space-y-3">
          <h3 className="text-sm font-semibold">逐项比较客户端格式</h3>
          <p className="text-muted-foreground text-xs leading-relaxed">查看候选配置后，在实际使用的客户端中与原订阅比较节点、入口和连接凭据，再手动勾选已完成的格式。可生成配置不表示已比较或已通过连接验证。</p>
          {formats.map(format => {
            const content = preview.formats[format]
            const supported = typeof content === 'string'
            return <div key={format} className="space-y-2 rounded-md border p-3">
              <div className="flex flex-wrap items-center gap-2"><strong className="mr-auto text-sm">{names[format]}</strong><Badge tone={compared.includes(format) ? 'good' : supported ? 'warm' : 'neutral'}>{compared.includes(format) ? '已人工比较' : supported ? '尚未比较' : '当前不可生成'}</Badge>{supported && <Button size="sm" variant="outline" disabled={busy} onClick={() => { setSelected(format); setViewed(previous => previous.includes(format) ? previous : [...previous, format]) }}>查看 {names[format]} 候选</Button>}</div>
              {!supported && <p className="text-muted-foreground text-xs">{preview.unavailable_formats[format] ?? (blocked ? '当前用户受限，保持不可获取订阅。' : '当前节点不能生成此格式。')}</p>}
              {supported && <label className="flex cursor-pointer items-center gap-3 py-1 text-sm"><Checkbox checked={compared.includes(format)} disabled={busy || !viewed.includes(format) || blocked} onCheckedChange={checked => changeCompared(format, checked === true)} /><span>已在客户端比较 {names[format]} 的节点、入口与凭据</span></label>}
            </div>
          })}
          {selected && typeof preview.formats[selected] === 'string' && <section aria-label={`${names[selected]} 候选配置`} className="space-y-2"><div className="flex flex-wrap items-center gap-2"><h4 className="mr-auto text-sm font-semibold">{names[selected]} 候选配置（含凭据）</h4><CopyButton text={preview.formats[selected]!} label={`复制 ${names[selected]} 候选`} /><Button size="sm" variant="ghost" onClick={() => setSelected(undefined)}>隐藏候选内容</Button></div><Textarea aria-label={`${names[selected]} 候选内容`} readOnly spellCheck={false} rows={8} className="max-h-80 font-mono text-xs" value={preview.formats[selected]} /></section>}
          {blocked && <label className="flex cursor-pointer items-center gap-3 py-2 text-sm"><Checkbox checked={compared.includes('blocked')} disabled={busy} onCheckedChange={checked => changeCompared('blocked', checked === true)} /><span>已确认原订阅与 apsp 同样不可用，暂停、到期或额度限制继续生效</span></label>}
        </section>
        <label className="flex cursor-pointer items-center gap-3 py-2 text-sm"><Checkbox checked={routeReady} disabled={busy || !addressVisible} onCheckedChange={checked => { setRouteReady(checked === true); setConfirmed(false) }} /><span>已验证原订阅入口代理到 apsp，旧地址继续可用</span></label>
        <label className="flex cursor-pointer items-center gap-3 py-2 text-sm"><Checkbox checked={confirmed} disabled={busy || !eligible || !selectedComparisons.length || !routeReady} onCheckedChange={checked => setConfirmed(checked === true)} /><span>已核对选中格式及影响，确认由 apsp 接管此用户</span></label>
        <p className="text-muted-foreground text-xs leading-relaxed">接管保留用户身份、节点凭据和已有用量；当前周期从采用时的用量衔接，之后按套餐周期计量。套餐、授权或部署变化会要求重新预览。未勾选格式尚未验收。</p>
      </>}
    </div>
    <footer><Button variant="outline" disabled={applying} onClick={close}>取消</Button><Button variant="outline" disabled={busy} onClick={() => setReload(value => value + 1)}>重新预览</Button><Button disabled={!canApply} onClick={() => void apply()}>{applying ? '正在接管…' : '确认接管此用户'}</Button></footer>
  </Modal>
}
