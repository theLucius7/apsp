import { useFormDraft } from '../control-center/preferences'
import { useRef, useState } from 'react'
import { api } from '../api'
import { ErrorNotice, Field, Modal } from '../components'
import { useAction } from '../hooks'
import { emptyMonitoring, withMonitoring, initialProbePayloads } from '../probes'
import type { Probe, ProbeMonitoring } from '../probes'
import ProbeMonitoringFields from './ProbeMonitoringFields'
import type { AgentSettings, Server } from '../types'

import ServerOperationsFields from './ServerOperationsFields'
import { assetDraft, assetPayload } from '../server-assets'
import ServerAssetFields, { SetupNavigation } from './ServerAssetFields'
import { Button } from '@/components/ui/button'
import { cn } from '@/lib/utils'
import { Input } from '@/components/ui/input'
import { NativeSelect } from '@/components/ui/native-select'
import { Spinner } from '@/components/ui/spinner'
import { Switch } from '@/components/ui/switch'
import { ActivityIcon, ArrowRightIcon, CheckIcon, PlusIcon, ServerIcon, WaypointsIcon } from 'lucide-react'

type ProbeDraft = Omit<Probe, 'id' | 'enabled' | 'port' | 'interval_secs'> & { key: number; port: string; interval: string; monitoring: ProbeMonitoring }
const frequencies = [
  { label: '实时', sample: '1', upload: '3', note: '及时观察变化' },
  { label: '均衡', sample: '3', upload: '10', note: '兼顾频率与开销' },
  { label: '轻量', sample: '10', upload: '30', note: '减少采集与请求' },
]

export function SetupSteps({ step }: { step: 1 | 2 }) {
  return <ol className="m-0 flex list-none gap-7 border-b p-0 pb-6 max-sm:gap-3.5 max-sm:pb-4" aria-label="添加服务器进度">
    <li className={cn('text-muted-foreground flex flex-1 items-center gap-2.5 max-sm:gap-2', step === 1 ? 'text-primary [&>span]:border-primary [&>span]:bg-primary [&>span]:text-primary-foreground [&>span]:ring-primary/10 [&>span]:ring-4' : 'text-success [&>span]:border-success/30 [&>span]:bg-success/10')} aria-current={step === 1 ? 'step' : undefined}><span className="bg-card grid size-7 flex-none place-items-center rounded-full border text-xs max-sm:size-6">{step === 2 ? <CheckIcon className="size-3.5" aria-hidden="true" /> : '1'}</span><div><strong className="block text-sm font-medium">配置服务器</strong><small className="mt-0.5 block text-xs">名称、监控与拨测</small></div></li>
    <li className={cn('text-muted-foreground flex flex-1 items-center gap-2.5 max-sm:gap-2', step === 2 && 'text-primary [&>span]:border-primary [&>span]:bg-primary [&>span]:text-primary-foreground [&>span]:ring-primary/10 [&>span]:ring-4')} aria-current={step === 2 ? 'step' : undefined}><span className="bg-card grid size-7 flex-none place-items-center rounded-full border text-xs max-sm:size-6">2</span><div><strong className="block text-sm font-medium">安装与接入</strong><small className="mt-0.5 block text-xs">复制命令，等待上线</small></div></li>
  </ol>
}

export default function ServerSetup({ onClose, onCreated }: { onClose: () => void; onCreated: (server: Server) => void }) {
  const action = useAction()
  const [name, setName] = useState('')
  const [asset, setAsset] = useState(() => assetDraft())
  const [sample, setSample] = useState('1'), [upload, setUpload] = useState('3')
  const [persist, setPersist] = useState('60')
  const [autoUpdate, setAutoUpdate] = useState(false), [discover, setDiscover] = useState(true)
  const [probes, setProbes] = useState<ProbeDraft[]>([])
  const draft = useFormDraft<{ name: string; sample: string; upload: string; persist: string; autoUpdate: boolean; discover: boolean }>('server-setup', Boolean(name || probes.length))
  const nextKey = useRef(0)
  const updateProbe = (key: number, change: Partial<ProbeDraft>) => setProbes(current => current.map(probe => {
    if (probe.key !== key) return probe
    const changed = { ...probe, ...change }
    const identityChanged = changed.target !== probe.target || changed.kind !== probe.kind || changed.port !== probe.port || changed.monitoring.ip_version !== probe.monitoring.ip_version
    return identityChanged ? { ...changed, monitoring: { ...changed.monitoring, authorization: { ...changed.monitoring.authorization, confirmed: false } } } : changed
  }))
  const addProbe = () => {
    const key = nextKey.current++
    setProbes(current => current.length >= 32 ? current : [...current, { key, name: '', kind: 'tcp', target: '', port: '443', interval: '30', carrier: '', monitoring: emptyMonitoring() }])
  }
  const submit = () => {
    const agent_settings: AgentSettings = { sample_interval_secs: Number(sample), upload_interval_secs: Number(upload), auto_update: autoUpdate, discover_public_ips: discover }
    const initialProbes: Probe[] = probes.map(probe => withMonitoring({ id: '00000000-0000-0000-0000-000000000000', name: probe.name.trim(), kind: probe.kind, target: probe.target.trim(), port: probe.kind === 'tcp' ? Number(probe.port) : null, interval_secs: Number(probe.interval), carrier: probe.carrier.trim(), enabled: true, monitor: null }, probe.monitoring))
    void action.run(() => api<Server>('/api/servers', 'POST', { name: name.trim(), agent_settings, telemetry_settings: { persist_interval_secs: Number(persist) }, probes: initialProbePayloads(initialProbes), asset_settings: assetPayload(asset) }), onCreated)
  }

  return <Modal title="添加服务器" onClose={onClose} busy={action.busy} className="sm:max-w-[52.5rem]">
    <form onSubmit={event => { event.preventDefault(); submit() }}>
      <SetupNavigation monitoring />
      <div className="bg-muted/40 px-4 py-5 sm:px-7 sm:py-6">
        <SetupSteps step={1} />
        <ErrorNotice message={draft.error} retry={draft.reload} />
        <div className="flex flex-wrap items-center gap-2.5"><Button variant="outline" type="button" disabled={!draft.ready || action.busy} onClick={() => void action.run(() => draft.save({ name, sample, upload, persist, autoUpdate, discover }))}>保存基础信息草稿</Button><Button variant="outline" type="button" disabled={!draft.ready || !draft.value || action.busy} onClick={() => { const value = draft.value; if (value) { setName(value.name); setSample(value.sample); setUpload(value.upload); setPersist(value.persist); setAutoUpdate(value.autoUpdate); setDiscover(value.discover) } }}>恢复草稿</Button><small>资产、拨测与凭据保持当前表单，草稿只保存基础信息和采集设置。</small></div>
        <div className="flex items-center gap-3.5 py-6 max-sm:gap-3 max-sm:py-5"><span className="bg-primary/10 text-primary border-primary/20 grid size-12 flex-none place-items-center rounded-xl border max-sm:size-10"><ServerIcon className="size-6" aria-hidden="true" /></span><div className="min-w-0 flex-1"><h3 className="text-lg font-semibold tracking-tight wrap-anywhere max-sm:text-base">连接一台新的服务器</h3><p className="text-muted-foreground mt-1.5 text-xs leading-relaxed">先设定监控方式，设备接入后自动同步。之后也可以在详情中调整。</p></div></div>
        <fieldset disabled={action.busy}>
          <section className="bg-card mb-4 scroll-mt-16 rounded-xl border p-5 last:mb-0 max-sm:p-4" aria-labelledby="setup-basics">
            <div className="mb-4 flex items-center justify-between gap-3.5 max-sm:flex-wrap"><div className="min-w-0"><h3 className="flex items-center gap-2 text-sm font-semibold" id="setup-basics">基础信息</h3><p className="text-muted-foreground mt-1 text-xs leading-relaxed">一个容易辨认的名称，就是接入的开始。</p></div><span className="bg-muted text-muted-foreground inline-block rounded border px-1.5 text-xs font-normal whitespace-nowrap">必填</span></div>
            <Field className="mb-0" label="服务器名称" hint="系统、架构和硬件信息会在 Agent 接入后自动获取。"><Input name="name" required pattern=".*\S.*" maxLength={128} value={name} onChange={event => setName(event.target.value)} placeholder="例如：东京 · 主节点" autoComplete="off" /></Field>
          </section>
          <ServerAssetFields value={asset} onChange={setAsset} />
      <ServerOperationsFields asset={asset} onChange={setAsset} />
          <section className="bg-card mb-4 scroll-mt-16 rounded-xl border p-5 last:mb-0 max-sm:p-4" aria-labelledby="setup-monitoring">
            <div className="mb-4 flex items-center justify-between gap-3.5 max-sm:flex-wrap"><div className="min-w-0"><h3 className="flex items-center gap-2 text-sm font-semibold" id="setup-monitoring">监控与采集</h3><p className="text-muted-foreground mt-1 text-xs leading-relaxed">采样、实时上报和历史写入分别设置，兼顾展示速度与数据库开销。</p></div><ActivityIcon className="size-5 text-muted-foreground flex-none" aria-hidden="true" /></div>
            <div className="mb-4 grid grid-cols-3 gap-2.5 max-sm:gap-1.5" role="group" aria-label="监控频率预设">{frequencies.map(frequency => <button className="bg-card hover:bg-accent aria-pressed:border-primary aria-pressed:bg-accent focus-visible:ring-ring/50 aria-pressed:after:bg-primary relative rounded-lg border px-3.5 py-3 text-left transition-colors outline-none focus-visible:ring-3 aria-pressed:after:absolute aria-pressed:after:top-3.5 aria-pressed:after:right-3 aria-pressed:after:size-1.5 aria-pressed:after:rounded-full max-sm:px-2 max-sm:py-2.5" key={frequency.label} type="button" aria-pressed={sample === frequency.sample && upload === frequency.upload} onClick={() => { setSample(frequency.sample); setUpload(frequency.upload) }}><strong className="mb-1.5 block text-sm font-semibold">{frequency.label}</strong><span className="block text-xs leading-relaxed">{frequency.sample} 秒采样 · {frequency.upload} 秒上传</span><small className="text-muted-foreground mt-1 block text-xs max-sm:hidden">{frequency.note}</small></button>)}</div>
            <div className="grid gap-4 sm:grid-cols-2">
              <Field className="mb-0" label="采样间隔（秒）" hint="每次采集系统指标的间隔，1–60 秒。"><Input name="sample_interval_secs" type="number" min={1} max={60} step={1} required value={sample} onChange={event => setSample(event.target.value)} /></Field>
              <Field className="mb-0" label="实时上报间隔（秒）" hint="应大于或等于采样间隔，最多 60 秒；不决定历史保存粒度。"><Input name="upload_interval_secs" type="number" min={Number(sample) || 1} max={60} step={1} required value={upload} onChange={event => setUpload(event.target.value)} /></Field>
              <Field className="mb-0" label="历史批量写入间隔（秒）" hint="15–3600 秒，默认 60 秒。新版 Agent 收到持久化确认后再清除本地缓存；旧 Agent 继续按原上传间隔写入。"><Input name="persist_interval_secs" type="number" min={15} max={3600} step={1} required value={persist} onChange={event => setPersist(event.target.value)} /></Field>
            </div>
            <div className="mt-5 border-t pt-1">
              <label className="relative flex cursor-pointer items-center gap-4 border-b py-3.5 last:border-0 last:pb-0"><span className="min-w-0 flex-1"><strong className="block text-sm font-medium">自动识别公网地址</strong><small className="text-muted-foreground mt-1 block text-xs leading-relaxed">识别 IPv4 / IPv6；设备本地关闭时，以本地设置为准。</small></span><Switch name="discover_public_ips" checked={discover} onCheckedChange={checked => setDiscover(checked)} /></label>
              <label className="relative flex cursor-pointer items-center gap-4 border-b py-3.5 last:border-0 last:pb-0"><span className="min-w-0 flex-1"><strong className="block text-sm font-medium">自动更新 Agent</strong><small className="text-muted-foreground mt-1 block text-xs leading-relaxed">从 GitHub 下载面板选定的兼容签名版本，保留设备身份与本地状态。</small></span><Switch name="auto_update" checked={autoUpdate} onCheckedChange={checked => setAutoUpdate(checked)} /></label>
            </div>
          </section>
          <section className="bg-card mb-4 scroll-mt-16 rounded-xl border p-5 last:mb-0 max-sm:p-4" aria-labelledby="setup-probes">
            <div className="mb-4 flex items-center justify-between gap-3.5 max-sm:flex-wrap"><div className="min-w-0"><h3 className="flex items-center gap-2 text-sm font-semibold" id="setup-probes">初始网络拨测 <span className="bg-muted text-muted-foreground inline-block rounded border px-1.5 text-xs font-normal whitespace-nowrap">可选</span></h3><p className="text-muted-foreground mt-1 text-xs leading-relaxed">接入后持续检测指定目标，结果显示在服务器展示页。统一延迟任务中的默认目标也会自动分配。</p></div><Button type="button" variant="outline" size="sm" onClick={addProbe} disabled={probes.length >= 32}><PlusIcon aria-hidden="true" />添加目标</Button></div>
            {!probes.length && <div className="bg-muted/40 flex items-center gap-4 rounded-lg border border-dashed p-5 max-sm:gap-3 max-sm:p-3.5"><WaypointsIcon className="size-6 text-muted-foreground flex-none max-sm:hidden" aria-hidden="true" /><div><strong className="text-sm font-medium">关心的线路，从接入时开始观察</strong><p className="text-muted-foreground mt-1.5 text-xs leading-relaxed">添加 TCP 目标查看延迟与连接失败率，或通过 ICMP 检测延迟与丢包。也可以稍后配置。</p></div></div>}
            {probes.map((probe, index) => <div key={probe.key} className="bg-muted/30 mt-3 rounded-lg border p-4 max-sm:p-3" role="group" aria-label={`拨测目标 ${index + 1}`}>
              <div className="mb-3.5 flex items-center justify-between"><strong className="text-muted-foreground text-xs font-medium tracking-wide">目标 {String(index + 1).padStart(2, '0')}</strong><Button type="button" variant="ghost" size="sm" className="text-destructive hover:bg-destructive/10 hover:text-destructive dark:hover:bg-destructive/20" onClick={() => setProbes(current => current.filter(item => item.key !== probe.key))} aria-label={`移除目标 ${index + 1}`}>移除</Button></div>
              <div className="grid gap-4 max-sm:grid-cols-2 sm:grid-cols-[minmax(0,1.4fr)_repeat(2,minmax(0,1fr))]">
                <Field className="mb-0" label="拨测名称"><Input required pattern=".*\S.*" maxLength={128} value={probe.name} onChange={event => updateProbe(probe.key, { name: event.target.value })} placeholder="例如：主站连通性" /></Field>
                <Field className="mb-0" label="检测方式"><NativeSelect className="w-full" value={probe.kind} onChange={event => updateProbe(probe.key, { kind: event.target.value as Probe['kind'] })}><option value="tcp">TCP 连接</option><option value="icmp">ICMP 回显</option></NativeSelect></Field>
                <Field className="mb-0" label="线路备注"><Input maxLength={64} value={probe.carrier} onChange={event => updateProbe(probe.key, { carrier: event.target.value })} placeholder="例如：电信 / 联通 / 移动" /></Field>
                <Field className="mb-0" label="目标地址"><Input required maxLength={253} value={probe.target} onChange={event => updateProbe(probe.key, { target: event.target.value })} placeholder="主机名或 IP，不含协议和路径" autoCapitalize="none" spellCheck={false} /></Field>
                {probe.kind === 'tcp' && <Field className="mb-0" label="目标端口"><Input type="number" min={1} max={65535} step={1} required value={probe.port} onChange={event => updateProbe(probe.key, { port: event.target.value })} /></Field>}
                <Field className="mb-0" label="拨测间隔（秒）"><Input type="number" min={10} max={3600} step={1} required value={probe.interval} onChange={event => updateProbe(probe.key, { interval: event.target.value })} /></Field>
              </div>
              <ProbeMonitoringFields value={probe.monitoring} onChange={monitoring => updateProbe(probe.key, { monitoring })} />
              <p className="text-muted-foreground mt-3 text-xs leading-relaxed wrap-anywhere">{probe.kind === 'icmp' ? '设备需具备 ICMP 检测权限；工具或权限不可用时会显示检测错误。' : 'TCP 通过建立连接测量可达性，连接失败率与 ICMP 丢包率分别展示。'}</p>
            </div>)}
            {probes.length > 0 && <p className="text-muted-foreground mt-3 text-xs leading-relaxed wrap-anywhere">已配置 {probes.length} / 32 个目标。已确认目标授权且新版 Agent 取得最长 90 秒执行许可后才开始调度。断连和冷启动不沿用许可，可在服务器详情中编辑、撤销授权或暂停。</p>}
          </section>
        </fieldset>
        <ErrorNotice message={action.error} />
      </div>
      <footer className="bg-popover sticky bottom-0 z-[1] flex flex-wrap items-center justify-end gap-2 border-t px-4 py-3 sm:px-7"><span className="text-muted-foreground min-w-32 flex-1 text-xs max-sm:mb-1 max-sm:basis-full">下一步：获取安装命令</span><Button type="button" variant="outline" disabled={action.busy} onClick={onClose}>取消</Button><Button type="submit" disabled={action.busy}>{action.busy ? <><Spinner role="presentation" aria-hidden="true" aria-label={undefined} />正在创建…</> : <>创建并继续<ArrowRightIcon aria-hidden="true" /></>}</Button></footer>
    </form>
  </Modal>
}
