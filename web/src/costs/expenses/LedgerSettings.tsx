import { useRef, useState } from 'react'
import { DownloadIcon, UploadIcon } from 'lucide-react'
import { api } from '../../api'
import { ErrorNotice, Field, Modal, SettingRow } from '../../components'
import { useAction, useResource } from '../../hooks'
import { ActionMenu } from '@/components/action-menu'
import { SegmentedButton, SegmentedNav } from '@/components/segmented'
import { Button } from '@/components/ui/button'
import { Checkbox } from '@/components/ui/checkbox'
import { Input } from '@/components/ui/input'
import { NativeSelect } from '@/components/ui/native-select'
import { TimezoneSelect } from '@/components/timezone-select'
import { Switch } from '@/components/ui/switch'
import { cn } from '@/lib/utils'
import { currencies, decimal, download, parseAmount, root } from './types'
import type { Label, Settings } from './types'

export type SettingsTab = 'general' | 'labels' | 'activity' | 'migration'
const tabs: [SettingsTab, string][] = [['general', '常规'], ['labels', '分类与付款'], ['activity', '提醒记录'], ['migration', '导入导出']]

function General({ onChanged }: { onChanged: () => void }) {
  const settings = useResource<Settings>(`${root}/settings`, 0)
  const action = useAction()
  const [saved, setSaved] = useState(false)
  const value = settings.data
  return <div className="grid gap-1">
    <ErrorNotice message={settings.error || action.error} retry={settings.reload} />
    {value && <form key={value.version} onChange={() => setSaved(false)} onSubmit={event => {
      event.preventDefault()
      if (!settings.isCurrent()) return
      const form = new FormData(event.currentTarget)
      void action.run(() => api(`${root}/settings`, 'PUT', { base_currency: String(form.get('currency')).toUpperCase(), budget_units: form.get('budget') ? parseAmount(form.get('budget')) : null, timezone: String(form.get('timezone')).trim(), reminders_enabled: form.get('reminders') === 'on', version: value.version }), () => { settings.reload(); onChanged(); setSaved(true) })
    }}>
      <div className="grid gap-x-4 sm:grid-cols-2">
        <Field label="统计币种" hint="每月费用和预算按这个币种折算。"><NativeSelect name="currency" defaultValue={value.base_currency}>{[...new Set([value.base_currency, ...currencies])].map(code => <option key={code} value={code}>{code}</option>)}</NativeSelect></Field>
        <Field label="每月预算" hint="留空表示不设预算。"><Input name="budget" inputMode="decimal" defaultValue={value.budget_units ? decimal(value.budget_units) : ''} placeholder="不设预算" /></Field>
      </div>
      <Field label="日期时区" hint="决定“今天”是哪一天，续费日按这个时区计算。"><TimezoneSelect name="timezone" required defaultValue={value.timezone} /></Field>
      <SettingRow title="续费提醒" description={<>总开关：打开后，开启了提醒的项目在续费前经「告警设置」的渠道通知；关闭时取消尚未发送的提醒。<a className="text-primary ml-1 underline-offset-4 hover:underline" href="#/alerts/settings">通知渠道</a></>}>
        <Switch name="reminders" defaultChecked={value.reminders_enabled} />
      </SettingRow>
      <div className="mt-4 flex items-center justify-end gap-3">{saved && <span role="status" className="text-success text-xs">已保存</span>}<Button disabled={action.busy || !settings.isCurrent()}>{action.busy ? '正在保存…' : '保存'}</Button></div>
    </form>}
  </div>
}

/** One kind of label: rename in place, stop using or delete, and add new ones at the bottom. */
function LabelGroup({ kind, title, labels, reload }: { kind: Label['kind']; title: string; labels: Label[]; reload: () => void }) {
  const action = useAction()
  const [renaming, setRenaming] = useState<{ id: number; name: string } | null>(null)
  const [draft, setDraft] = useState('')
  const input = useRef<HTMLInputElement>(null)
  const write = (path: string, method: string, body?: unknown) => void action.run(() => api(path, method, body), () => { setRenaming(null); setDraft(''); reload() })
  // An empty name only returns to the field; the button itself stays available.
  const add = () => { if (draft.trim()) write(`${root}/labels`, 'POST', { kind, name: draft.trim(), enabled: true }); else input.current?.focus() }
  const own = labels.filter(label => label.kind === kind)
  return <section aria-label={title} className="grid content-start gap-2">
    <h3 className="text-xs font-medium">{title}<span className="text-muted-foreground ml-1.5 font-normal tabular-nums">{own.length}</span></h3>
    <ErrorNotice message={action.error} />
    <ul className="divide-y rounded-lg border text-[13px]">
      {own.map(label => <li key={label.id} className="flex min-h-10 items-center gap-2 px-3 py-1.5">
        {renaming?.id === label.id ? <form className="flex flex-1 gap-2" onSubmit={event => { event.preventDefault(); if (renaming.name.trim()) write(`${root}/labels/${label.id}`, 'PUT', { kind, name: renaming.name.trim(), enabled: label.enabled }) }}>
          <Input aria-label={`重命名 ${label.name}`} autoFocus maxLength={100} value={renaming.name} onChange={event => setRenaming({ id: label.id, name: event.target.value })} onKeyDown={event => { if (event.key === 'Escape') setRenaming(null) }} />
          <Button size="sm" disabled={action.busy}>保存</Button>
        </form> : <>
          <span className={cn('min-w-0 flex-1 truncate', !label.enabled && 'text-muted-foreground')}>{label.name}{!label.enabled && <span className="ml-1.5 text-xs">已停用</span>}</span>
          <ActionMenu label={`${label.name} 的操作`} groups={[[{ label: '重命名', onSelect: () => setRenaming({ id: label.id, name: label.name }) }, { label: label.enabled ? '停用' : '恢复使用', disabled: action.busy, onSelect: () => write(`${root}/labels/${label.id}`, 'PUT', { kind, name: label.name, enabled: !label.enabled }) }], [{ label: '删除', destructive: true, disabled: action.busy, onSelect: () => write(`${root}/labels/${label.id}`, 'DELETE') }]]} />
        </>}
      </li>)}
      <li className="flex gap-2 px-3 py-2">
        <Input ref={input} aria-label={`新的${title}`} placeholder={`添加${title}`} maxLength={100} value={draft} onChange={event => setDraft(event.target.value)} onKeyDown={event => { if (event.key === 'Enter') { event.preventDefault(); add() } }} />
        <Button variant="outline" size="sm" onClick={add}>添加</Button>
      </li>
    </ul>
  </section>
}

function Labels({ onChanged }: { onChanged: () => void }) {
  const labels = useResource<{ entries: Label[] }>(`${root}/labels`, 0)
  const reload = () => { labels.reload(); onChanged() }
  return <div className="grid gap-4">
    <ErrorNotice message={labels.error} retry={labels.reload} />
    {labels.data && <div className="grid gap-4 md:grid-cols-3">
      <LabelGroup kind="category" title="分类" labels={labels.data.entries} reload={reload} />
      <LabelGroup kind="payment_method" title="付款方式" labels={labels.data.entries} reload={reload} />
      <LabelGroup kind="payer" title="付款人" labels={labels.data.entries} reload={reload} />
    </div>}
    <p className="text-muted-foreground text-xs">还有项目在用的不能删除，可以停用：停用后不再出现在新项目的选项里。</p>
  </div>
}

type Activity = { id: string; kind: string; created_at: number; detail: { name?: string; due?: string }; deliveries: { channel: string; state: string; attempts: number; last_error: string | null }[] }
const activityKinds: Record<string, string> = { renewal_reminder: '续费提醒', schedule_advanced: '自动进入下一期', cancelled_on_date: '到取消日停用', manual_renewal: '确认续期', created: '添加项目', updated: '修改项目', deleted: '删除项目', cloned: '复制项目', imported: '导入', settings_updated: '修改设置' }
const deliveryStates: Record<string, string> = { pending: '待发送', sending: '发送中', sent: '已发送', failed: '发送失败', cancelled: '已取消' }

function ActivityLog() {
  const data = useResource<{ entries: Activity[] }>(`${root}/notifications`, 30_000)
  return <div className="grid gap-2">
    <ErrorNotice message={data.error} retry={data.reload} />
    {!data.data ? !data.error && <p className="text-muted-foreground text-sm">正在读取…</p> : data.data.entries.length === 0 ? <p className="text-muted-foreground text-sm">还没有提醒或改动。</p>
      : <ul className="divide-y rounded-lg border text-[13px]">{data.data.entries.map(entry => <li key={entry.id} className="grid gap-0.5 px-3 py-2">
        <div className="flex flex-wrap items-baseline justify-between gap-x-3"><span>{activityKinds[entry.kind] ?? entry.kind}{entry.detail.name && <span className="text-muted-foreground"> · {entry.detail.name}{entry.detail.due ? ` · ${entry.detail.due}` : ''}</span>}</span><time className="text-muted-foreground text-xs tabular-nums">{new Date(entry.created_at * 1000).toLocaleString('zh-CN', { hour12: false })}</time></div>
        {entry.deliveries.map(delivery => <small key={delivery.channel} className={cn('text-xs', delivery.state === 'failed' ? 'text-destructive' : 'text-muted-foreground')}>{delivery.channel === 'telegram' ? 'Telegram' : 'Webhook'} · {deliveryStates[delivery.state] ?? delivery.state}{delivery.attempts > 1 ? ` · 第 ${delivery.attempts} 次` : ''}{delivery.last_error ? ` · ${delivery.last_error}` : ''}</small>)}
      </li>)}</ul>}
  </div>
}

type Preview = { digest: string; already_imported: boolean; subscriptions?: number; labels?: number; warnings?: string[] }
function Migration({ onChanged }: { onChanged: () => void }) {
  const action = useAction()
  const picker = useRef<HTMLInputElement>(null)
  const [fileName, setFileName] = useState('')
  const [snapshot, setSnapshot] = useState<unknown>()
  const [preview, setPreview] = useState<Preview>()
  const [done, setDone] = useState(false)
  const [checked, setChecked] = useState(false)
  const choose = (file?: File) => {
    setSnapshot(undefined); setPreview(undefined); setDone(false); setChecked(false); setFileName(file?.name ?? '')
    if (file) void action.run(async () => {
      if (file.size > 8 * 1024 * 1024) throw new Error('配置文件不能超过 8 MiB。')
      const value: unknown = JSON.parse(await file.text())
      const result = await api<Preview>(`${root}/import/preview`, 'POST', value)
      setSnapshot(value); setPreview(result)
    })
  }
  return <div className="grid gap-5">
    <section className="grid gap-2">
      <h3 className="text-xs font-medium">导入</h3>
      <p className="text-muted-foreground text-xs leading-relaxed">选择 apsp 导出的文件，或用仓库里的 Wallos 只读导出工具生成的文件；不需要上传数据库或凭据。同一份文件重复导入不会重复添加。</p>
      <div className="flex flex-wrap items-center gap-3">
        <input ref={picker} type="file" accept="application/json,.json" className="sr-only" tabIndex={-1} aria-label="选择配置文件" onChange={event => { choose(event.target.files?.[0]); event.target.value = '' }} />
        <Button variant="outline" disabled={action.busy} onClick={() => picker.current?.click()}><UploadIcon aria-hidden="true" />选择文件</Button>
        <span className="text-muted-foreground min-w-0 truncate text-xs">{fileName || '还没有选择文件'}</span>
      </div>
      <ErrorNotice message={action.error} />
      {preview && <div data-slot="import-preview" className="bg-muted/40 grid gap-2 rounded-lg border p-3.5 text-sm">
        <p>{preview.already_imported ? '这份文件已经导入过。' : `将添加 ${preview.subscriptions ?? 0} 个项目，对应 ${preview.labels ?? 0} 个分类与付款方式。`}</p>
        {preview.warnings?.map(warning => <p key={warning} className="text-warning text-xs">{warning}</p>)}
        {!preview.already_imported && !done && <>
          <label className="flex items-center gap-2.5 text-xs"><Checkbox checked={checked} onCheckedChange={value => setChecked(value === true)} />已核对；导入后续费提醒总开关会关闭，核对完再打开</label>
          <div><Button disabled={action.busy || !checked} onClick={() => void action.run(() => api(`${root}/import`, 'POST', { snapshot, confirmed_digest: preview.digest }), () => { setDone(true); onChanged() })}><UploadIcon aria-hidden="true" />导入</Button></div>
        </>}
        {done && <p role="status" className="text-success">导入完成，请在列表里核对日期、金额和分类。</p>}
      </div>}
    </section>
    <section className="grid gap-2">
      <h3 className="text-xs font-medium">导出</h3>
      <p className="text-muted-foreground text-xs">全部项目、分类与设置导出为一个 JSON 文件，可以在别的 apsp 里导入。</p>
      <div><Button variant="outline" disabled={action.busy} onClick={() => void action.run(() => download(`${root}/export`, 'apsp-expenses.json'))}><DownloadIcon aria-hidden="true" />导出</Button></div>
    </section>
  </div>
}

/** Everything that is set once and rarely changed: currency and budget, labels, reminder history, import and export. */
export default function LedgerSettings({ initial = 'general', onClose, onChanged }: { initial?: SettingsTab; onClose: () => void; onChanged: () => void }) {
  const [tab, setTab] = useState<SettingsTab>(initial)
  return <Modal title="费用与续费设置" onClose={onClose} wide slot="ledger-settings" className="sm:max-w-3xl">
    <div data-slot="modal-body" className="grid gap-4 p-4">
      <SegmentedNav aria-label="设置分页" className="-mt-1">{tabs.map(([value, label]) => <SegmentedButton key={value} aria-pressed={tab === value} onClick={() => setTab(value)}>{label}</SegmentedButton>)}</SegmentedNav>
      {tab === 'general' && <General onChanged={onChanged} />}
      {tab === 'labels' && <Labels onChanged={onChanged} />}
      {tab === 'activity' && <ActivityLog />}
      {tab === 'migration' && <Migration onChanged={onChanged} />}
    </div>
    <footer><Button variant="outline" onClick={onClose}>完成</Button></footer>
  </Modal>
}
