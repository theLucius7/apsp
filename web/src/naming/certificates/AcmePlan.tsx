import { useEffect, useState } from 'react'
import { CircleAlertIcon, InfoIcon } from 'lucide-react'
import { api } from '../../api'
import { Field } from '../../components'
import { resourceWriteError, useResource } from '../../hooks'
import { time } from '../../format'
import { Alert, AlertDescription } from '@/components/ui/alert'
import { Button } from '@/components/ui/button'
import { Checkbox } from '@/components/ui/checkbox'
import { Input } from '@/components/ui/input'
import { Panel } from '@/components/panel'

export type Plan = {
  tool_version: string
  credential_id: string
  email: string
  staging: boolean
  terms_accepted: boolean
  license_accepted: boolean
  automatic_renewal: boolean
  renew_before_days: number
}

export type ToolVersionMismatch = {
  code?: string
  configured_tool_version?: string
  builtin_tool_version?: string
  remediation?: string
}

export type Current = {
  plan: { config: Plan; revision: number; status: string; next_run_at: number; updated_at?: number } | null
  tool?: string
  builtin_tool_version?: string
  default_tool_version?: string
  tool_path?: string
  tool_image?: string
  license?: string
  tool_source?: string
  tool_supply?: string
  deployment?: string
  private_key?: string
  tool_version_mismatch?: ToolVersionMismatch | null
}

type Job = {
  id: string
  status: string
  created_at: number
  result: {
    issued?: boolean | string
    saved?: boolean
    deployed?: boolean
    handshake_verified?: boolean
    error_code?: string
    error?: string
    cleanup_required?: boolean
    tool_version?: string
    requested_tool_version?: string
    builtin_tool_version?: string
  } | null
}

export function defaultAcmePlan(toolVersion = ''): Plan {
  return {
    tool_version: toolVersion,
    credential_id: '',
    email: '',
    staging: true,
    terms_accepted: false,
    license_accepted: false,
    automatic_renewal: false,
    renew_before_days: 30,
  }
}

export function resolveBuiltinToolVersion(current?: Current | null): string {
  return current?.builtin_tool_version?.trim()
    || current?.default_tool_version?.trim()
    || current?.tool_version_mismatch?.builtin_tool_version?.trim()
    || ''
}

export function syncAcmePlanDraft(
  state: { plan: Plan; editingRevision: number | null; dirty: boolean },
  current?: Current | null,
): { plan: Plan; editingRevision: number | null } {
  if (!current) return { plan: state.plan, editingRevision: state.editingRevision }
  const builtin = resolveBuiltinToolVersion(current)
  if (!state.dirty) {
    if (current.plan) return { plan: current.plan.config, editingRevision: current.plan.revision }
    const nextPlan = state.plan.tool_version === builtin ? state.plan : { ...state.plan, tool_version: builtin }
    return { plan: nextPlan, editingRevision: null }
  }
  // Fill only an empty tool_version if the administrator edited other fields before the first GET completed.
  if (!state.plan.tool_version) {
    const initialVersion = current.plan?.config.tool_version?.trim() || builtin
    if (initialVersion) return { plan: { ...state.plan, tool_version: initialVersion }, editingRevision: state.editingRevision }
  }
  return { plan: state.plan, editingRevision: state.editingRevision }
}

export function describeToolVersionMismatch(
  current: Current | undefined,
  draftToolVersion: string,
): {
  builtinVersion: string
  configuredVersion: string
  draftMismatch: boolean
  savedMismatch: boolean
  remediation: string
} {
  const builtinVersion = resolveBuiltinToolVersion(current)
  const serverMismatch = current?.tool_version_mismatch ?? null
  const savedVersion = serverMismatch?.configured_tool_version?.trim()
    || current?.plan?.config.tool_version?.trim()
    || ''
  const draftVersion = draftToolVersion.trim()
  const draftMismatch = Boolean(builtinVersion && draftVersion && draftVersion !== builtinVersion)
  const savedMismatch = Boolean(
    serverMismatch || (builtinVersion && savedVersion && savedVersion !== builtinVersion),
  )
  const configuredVersion = draftMismatch ? draftVersion : savedVersion
  const remediation = serverMismatch?.remediation?.trim()
    || (configuredVersion && builtinVersion
      ? `签发计划固定的 lego 版本 ${configuredVersion} 与当前面板内置官方 lego ${builtinVersion} 不匹配；请将签发计划中的固定 lego 版本更新为 ${builtinVersion} 并保存后重试`
      : '')
  return { builtinVersion, configuredVersion, draftMismatch, savedMismatch, remediation }
}

export default function AcmePlan({ certificateId, run }: { certificateId: string; run: (work: () => Promise<unknown>) => Promise<void> }) {
  const current = useResource<Current>(`/api/network-configuration/certificates/${certificateId}/acme-plan`)
  const jobs = useResource<Job[]>(`/api/network-configuration/certificates/${certificateId}/issuance`)
  const [activeCertificateId, setActiveCertificateId] = useState(certificateId)
  const [plan, setPlan] = useState<Plan>(defaultAcmePlan)
  const [confirmed, setConfirmed] = useState(false), [reconciled, setReconciled] = useState(false)
  const [dirty, setDirty] = useState(false), [editingRevision, setEditingRevision] = useState<number | null>(null)
  if (activeCertificateId !== certificateId) {
    setActiveCertificateId(certificateId)
    setPlan(defaultAcmePlan())
    setConfirmed(false)
    setReconciled(false)
    setDirty(false)
    setEditingRevision(null)
  }
  useEffect(() => {
    if (!current.fresh || !current.data) return
    setPlan(previous => syncAcmePlanDraft({ plan: previous, editingRevision, dirty }, current.data).plan)
    if (!dirty) setEditingRevision(current.data.plan?.revision ?? null)
  }, [current.data, current.fresh, dirty, editingRevision])
  const change = (part: Partial<Plan>) => { setDirty(true); setPlan(value => ({ ...value, ...part })) }
  const mismatch = describeToolVersionMismatch(current.data, plan.tool_version)
  const toolName = current.data?.tool?.trim() || 'lego'
  const versionLabel = mismatch.builtinVersion
    ? `${toolName} ${mismatch.builtinVersion}`
    : current.loading && !current.data
      ? '正在读取面板内置版本…'
      : '未读取到面板内置版本'
  return <Panel className="grid gap-3 p-5"><h3 className="text-sm font-semibold">DNS-01 自动签发与续期</h3><p className="text-muted-foreground mt-3 text-xs leading-relaxed">使用面板受控固定版本的 lego，向 Cloudflare 与 Let's Encrypt 发起真实请求。账户材料与私钥以加密凭据保存；不会自动部署到目标服务。默认使用预发布签发环境。</p>
    {current.error && <p className="text-destructive text-sm" role="alert">{current.error}</p>}
    <div data-slot="acme-builtin-tool" className="bg-muted/40 grid gap-1 rounded-lg border px-3.5 py-2.5 text-sm">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <span className="font-medium">固定 lego 版本</span>
        <code className="font-mono text-xs font-semibold">{versionLabel}</code>
      </div>
      <p className="text-muted-foreground text-xs leading-relaxed">
        {current.data?.tool_supply === 'panel_builtin_verified_binary' || Boolean(mismatch.builtinVersion)
          ? '标准面板镜像自带固定版官方 lego，无需另选或安装工具。'
          : '读取面板内置签发工具信息后自动填入版本。'}
        {mismatch.draftMismatch && <> 当前草稿仍固定旧版本 <code>{plan.tool_version}</code>。</>}
      </p>
    </div>
    {mismatch.draftMismatch ? <Alert variant="warning" data-slot="acme-tool-version-mismatch">
      <CircleAlertIcon aria-hidden="true" />
      <AlertDescription className="text-foreground">
        <p className="break-words">{mismatch.remediation}</p>
        <div className="mt-2 flex flex-wrap items-center gap-2">
          <Button type="button" size="sm" variant="outline" disabled={!mismatch.builtinVersion} onClick={() => change({ tool_version: mismatch.builtinVersion })}>更新为当前内置版本（{mismatch.builtinVersion}）</Button>
        </div>
      </AlertDescription>
    </Alert> : mismatch.savedMismatch && <Alert variant="info" role="status" data-slot="acme-tool-version-updated">
      <InfoIcon aria-hidden="true" />
      <AlertDescription className="text-foreground">草稿已更新为当前面板内置 {toolName} 版本 <code>{mismatch.builtinVersion}</code>{mismatch.configuredVersion ? <>（原已保存版本 <code>{mismatch.configuredVersion}</code>）</> : ''}，未保存的其他字段已保留；请点击「保存签发与续期计划」完成更新。</AlertDescription>
    </Alert>}
    <Field label="Cloudflare DNS 凭据中心标识"><Input value={plan.credential_id} onChange={event => change({ credential_id: event.target.value })} placeholder="受限 DNS 账号凭据的标识" /></Field>
    <Field label="签发联系邮箱"><Input type="email" value={plan.email} onChange={event => change({ email: event.target.value })} /></Field>
    <Field label="自动续期提前天数"><Input type="number" min={7} max={60} value={plan.renew_before_days} onChange={event => change({ renew_before_days: Number(event.target.value) })} /></Field>
    <label className="flex items-center gap-2 text-sm"><Checkbox checked={plan.staging} onCheckedChange={checked => change({ staging: checked === true })} />使用 Let's Encrypt 预发布环境；该证书不受普通客户端信任</label>
    <label className="flex items-center gap-2 text-sm"><Checkbox checked={plan.terms_accepted} onCheckedChange={checked => change({ terms_accepted: checked === true })} />已确认所选签发服务的条款</label>
    <label className="flex items-center gap-2 text-sm"><Checkbox checked={plan.license_accepted} onCheckedChange={checked => change({ license_accepted: checked === true })} />已确认 lego 的 MIT 许可及所使用 DNS 服务条款</label>
    <label className="flex items-center gap-2 text-sm"><Checkbox checked={plan.automatic_renewal} onCheckedChange={checked => change({ automatic_renewal: checked === true })} />授权自动首次签发，并在证书进入续期窗口时签发新版本</label>
    {dirty && current.data !== undefined && (current.data.plan?.revision ?? null) !== editingRevision && <p className="text-destructive text-sm" role="alert">签发计划已被并发修改，当前草稿保留；请重新核对远端配置后保存。</p>}
    <Button type="button" variant="outline" disabled={!current.fresh || !plan.tool_version || !plan.credential_id || !plan.email || !plan.license_accepted || !plan.terms_accepted} onClick={() => void run(async () => { const error = resourceWriteError(current); if (error) throw new Error(error); await api(`/api/network-configuration/certificates/${certificateId}/acme-plan`, 'PUT', { config: plan, revision: editingRevision }); current.reload(); jobs.reload(); setDirty(false) })}>保存签发与续期计划</Button>
    <label className="flex items-center gap-2 text-sm"><Checkbox checked={confirmed} onCheckedChange={checked => setConfirmed(checked === true)} />确认本次真实签发及 DNS TXT 创建与清理</label>
    {jobs.data?.some(job => job.status === 'unknown') && <label className="flex items-center gap-2 text-sm"><Checkbox checked={reconciled} onCheckedChange={checked => setReconciled(checked === true)} />已在签发方与 DNS 提供方人工核对未知请求，并处理遗留 TXT 后允许重新下单</label>}
    <div className="my-3.5 flex flex-wrap items-center gap-2.5"><Button type="button" disabled={!confirmed || !current.fresh || !jobs.fresh || !current.data?.plan || mismatch.savedMismatch || mismatch.draftMismatch || (jobs.data?.some(job => job.status === 'unknown') && !reconciled)} onClick={() => void run(async () => { const error = resourceWriteError(current, jobs); if (error) throw new Error(error); await api(`/api/network-configuration/certificates/${certificateId}/issue`, 'POST', { confirmed, remote_reconciled: reconciled }); jobs.reload(); return { status: '签发已排队，等待后台执行器结果' } })}>单独发起签发</Button><Button type="button" variant="outline" onClick={() => { current.reload(); jobs.reload() }}>刷新签发阶段</Button></div>
    {jobs.error && <p className="text-destructive text-sm" role="alert">{jobs.error}</p>}
    {jobs.data?.map(job => <article key={job.id}><strong>{({ queued: '已排队', running: '正在签发', succeeded: '已签发并保存', failed: '执行前失败，尚未签发', unknown: '执行结果未知，需核对', reconciled: '已人工核对' } as Record<string, string>)[job.status] ?? '状态未知'}</strong><span> · {time(job.created_at)}</span><p>签发 {job.result?.issued === true ? '已完成' : job.result?.issued === 'unknown' ? '未知' : '待完成'} · 保存 {job.result?.saved ? '已完成' : '待完成'} · 部署 {job.result?.deployed ? '已完成' : '维护方待执行'} · 握手 {job.result?.handshake_verified ? '已验证' : '待验证'}</p>{job.result?.error_code && <code>{job.result.error_code}</code>}{job.result?.error && <p className="text-destructive text-xs leading-relaxed">{job.result.error}</p>}{job.result?.cleanup_required && <p className="text-destructive text-sm" role="alert">需要在提供方核对遗留的验证记录；系统不会自动重放未知请求。</p>}</article>)}
  </Panel>
}
