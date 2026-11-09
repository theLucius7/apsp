import { useRef, useState } from 'react'
import { api } from '../../api'
import { Confirm, Empty, ErrorNotice, Field, FormDialog, Loading, PageHeader, Refresh, DialogPresence, InfoTip } from '../../components'
import { bytes } from '../../format'
import { resourceWriteError, useAction, useResource } from '../../hooks'
import { publicPathText, quotaBytes, scheduleText, validatedSnapshot, validProxyResources } from './groupTypes'
import type { ProxyResource, PackageGroup, PolicyGroup } from './groupTypes'
import { endpoint } from './resourceTypes'
import { Button, buttonVariants } from '@/components/ui/button'
import { Checkbox } from '@/components/ui/checkbox'
import { Input } from '@/components/ui/input'
import { NativeSelect } from '@/components/ui/native-select'
import { TimezoneSelect } from '@/components/timezone-select'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'
import { ChipList } from '@/components/chip-list'
import { ActivityIcon, WaypointsIcon } from 'lucide-react'
import { SegmentedButton, SegmentedGroup } from '@/components/segmented'
import { Panel, PanelHeader, PanelTitle } from '@/components/panel'
import GroupExternalNodes, { externalSelectionError } from './GroupExternalNodes'
import BatchProxyAssignments from './BatchProxyAssignments'
import type { ProxyUser } from '../../types'
import { externalReference } from './externalAccessTypes'
import type { ExternalEntry, ExternalReference } from './externalAccessTypes'

const root = '/api/plugins/sing-box'
type Tab = 'policy-groups' | 'package-groups'
type EditorInput = { kind: 'policy-groups'; value?: PolicyGroup } | { kind: 'package-groups'; value?: PackageGroup }
type Editor = { kind: 'policy-groups'; value?: PolicyGroup; nodeIds: number[]; chainIds: number[]; externals: ExternalReference[] } | { kind: 'package-groups'; value?: PackageGroup }
const labels: Record<Tab, string> = { 'policy-groups': '策略组', 'package-groups': '套餐组' }

type Selectable = { kind: 'direct' | 'chain'; id: number; name: string; available: boolean; description: string }
/** What a policy group can hold: managed nodes and chains, each described by where it connects. */
function selectable(resources: ProxyResource[]): Selectable[] {
  return resources.map(resource => ({ kind: resource.kind, id: resource.id, name: resource.name, available: resource.available,
    description: resource.kind === 'direct' ? `${resource.entry.server_name} · ${endpoint(resource.entry.public_host, resource.entry.public_port)}` : `${resource.entry.name} → ${publicPathText(resource.hops)}` }))
}

export default function Groups({ initialTab = 'policy-groups' }: { initialTab?: Tab } = {}) {
  const policies = useResource<PolicyGroup[]>(`${root}/policy-groups`)
  const users = useResource<ProxyUser[]>(`${root}/users`, 30_000, { keep: true })
  const packages = useResource<PackageGroup[]>(`${root}/package-groups`)
  const resourceQuery = useResource<unknown>(`${root}/ordered-proxy-resources`)
  const externalQuery = useResource<{ available_nodes: ExternalEntry[] }>(`${root}/external-node-candidates`)
  const candidates = externalQuery.data?.available_nodes ?? []
  const resourceHistory = useRef<ProxyResource[] | undefined>(undefined)
  const resources = validatedSnapshot(resourceQuery, validProxyResources, resourceHistory.current)
  if (resources.fresh) resourceHistory.current = resources.data
  const inventory = selectable(resources.data ?? [])
  const nodes = inventory.filter(resource => resource.kind === 'direct')
  const chains = inventory.filter(resource => resource.kind === 'chain')
  const currentInventory = () => selectable(resources.getCurrent?.() ?? [])
  const action = useAction()
  const [tab, setTab] = useState<Tab>(initialTab)
  const [editor, setEditor] = useState<Editor | null>(null)
  const [deleting, setDeleting] = useState<{ kind: Tab; id: number; name: string } | null>(null)
  // External candidates block a save only when they failed to load: the server checks every reference again.
  const externalState = { error: externalQuery.error, isCurrent: () => true }
  const policyWriteError = resourceWriteError(policies, packages, resources, externalState)
  const packageWriteError = resourceWriteError(policies, packages, resources)
  const writeError = (kind: Tab, id?: number) => {
    // Policy groups also hold external nodes, so their candidates must have loaded.
    const dependencyError = kind === 'policy-groups' ? resourceWriteError(policies, packages, resources, externalState) : resourceWriteError(policies, packages, resources)
    if (dependencyError) return dependencyError
    const data = kind === 'policy-groups' ? policies.getCurrent() : packages.getCurrent()
    return id !== undefined && !data?.some(value => value.id === id) ? '此资源已不可用，暂不能提交。草稿已保留，可关闭窗口后重新选择。' : ''
  }
  // Migrated lines the group already holds stay selectable while the old panel runs their host.
  const heldMigrated = editor?.kind === 'policy-groups' ? editor.value?.migrated_node_ids ?? [] : []
  const invalidNodes = editor?.kind === 'policy-groups' ? editor.nodeIds.filter(id => !heldMigrated.includes(id) && !nodes.some(node => node.id === id && node.available)) : []
  const invalidChains = editor?.kind === 'policy-groups' ? editor.chainIds.filter(id => !chains.some(chain => chain.id === id && chain.available)) : []
  const currentSelectionError = () => editor?.kind === 'policy-groups' && [...editor.nodeIds.filter(id => !heldMigrated.includes(id)).map(id => ({ kind: 'direct', id })), ...editor.chainIds.map(id => ({ kind: 'chain', id }))].some(selected => !currentInventory().some(value => value.kind === selected.kind && value.id === selected.id && value.available)) ? '已选资源已不可用或身份已变更，请取消这些选择后再保存；其余草稿已保留。' : editor?.kind === 'policy-groups' ? externalSelectionError(editor.externals, policies.getCurrent()?.find(group => group.id === editor.value?.id)?.external_nodes ?? [], externalQuery.getCurrent()?.available_nodes ?? []) : ''
  const selectionError = invalidNodes.length || invalidChains.length ? '已选资源已不可用或身份已变更，请取消这些选择后再保存。其余草稿已保留。' : editor?.kind === 'policy-groups' ? externalSelectionError(editor.externals, editor.value?.external_nodes ?? [], candidates) : ''
  const refresh = () => { policies.reload(); packages.reload(); resourceQuery.reload(); externalQuery.reload(); users.reload() }
  const nodeName = (id: number) => nodes.find(n => n.id === id)?.name ?? `节点 #${id}（已不可用）`
  const open = (value: EditorInput) => {
    if (writeError(value.kind, value.value?.id)) return
    action.clearError()
    setEditor(value.kind === 'policy-groups' ? { ...value, nodeIds: [...(value.value?.node_ids ?? [])], chainIds: [...(value.value?.chain_ids ?? [])], externals: (value.value?.external_nodes ?? []).map(externalReference) } : value)
  }
  const choose = (kind: 'nodeIds' | 'chainIds', id: number, checked: boolean) => { if (resourceWriteError(policies, packages, resources)) return; setEditor(previous => previous?.kind === 'policy-groups' ? { ...previous, [kind]: checked ? [...previous[kind], id] : previous[kind].filter(value => value !== id) } : previous) }
  const remove = (kind: Tab, value: { id: number; name: string }) => { if (writeError(kind, value.id)) return; action.clearError(); setDeleting({ kind, ...value }) }
  const submit = (form: FormData) => {
    if (!editor || writeError(editor.kind, editor.value?.id) || currentSelectionError()) return
    void action.run(async () => {
      const name = String(form.get('name') ?? '').trim()
      const value = 'value' in editor ? editor.value : undefined
      const body = editor.kind === 'policy-groups' ? { name, node_ids: editor.nodeIds, chain_ids: editor.chainIds, external_nodes: editor.externals }
        : { name, monthly_bytes: quotaBytes(String(form.get('amount') ?? ''), String(form.get('unit'))), reset_day: Number(form.get('reset_day')), reset_hour: Number(String(form.get('reset_time')).split(':')[0]), reset_minute: Number(String(form.get('reset_time')).split(':')[1]), timezone: String(form.get('timezone')), duration_days: Number(form.get('duration_days')) }
      return api(`${root}/${editor.kind}${value ? `/${value.id}` : ''}`, value ? 'PUT' : 'POST', body)
    }, () => { setEditor(null); refresh() })
  }
  return <>
    <PageHeader title="策略与套餐"><Refresh onClick={refresh} /><a className={buttonVariants({ variant: 'outline' })} href="#/nodes">管理代理节点</a><a className={buttonVariants({ variant: 'outline' })} href="#/users">分配给代理用户</a></PageHeader>
    <SegmentedGroup className="mb-4" role="group" aria-label="管理内容">{(Object.keys(labels) as Tab[]).map(key => <SegmentedButton type="button" key={key} aria-pressed={tab === key} onClick={() => setTab(key)}>{labels[key]}</SegmentedButton>)}</SegmentedGroup>
    <ErrorNotice message={policies.error || packages.error || resources.error || externalQuery.error} retry={refresh} />
    <Panel>
      {writeError(tab) && <p className="text-muted-foreground mt-3 text-xs leading-relaxed" role="status">{writeError(tab)}</p>}
      <PanelHeader><span className="flex min-w-0 items-center gap-1.5"><PanelTitle>{labels[tab]}</PanelTitle><InfoTip label={tab === 'policy-groups' ? '授权规则' : '套餐规则'}><p>{tab === 'policy-groups' ? '用户只通过策略组获得节点。多个策略组取并集，只取消一个组，不会撤销其他组仍授予的节点。「原权限组」由原系统的权限组转来，组内迁移线路沿用用户原有的客户端身份。' : '套餐修改仅影响之后的分配；已分配用户保留原有套餐快照。29 至 31 日在短月份按月末重置，下一月仍按原设定日期计算。'}</p></InfoTip></span><Button size="sm" onClick={() => open({ kind: tab })} disabled={action.busy || Boolean(writeError(tab))}>创建{labels[tab]}</Button></PanelHeader>
      {tab === 'policy-groups' && (policies.loading && !policies.data ? <Loading variant="table" rows={3} /> : !policies.data?.length ? <Empty icon={WaypointsIcon} title="把常用节点放进一个策略组" description="一个用户可以有多个策略组；修改组内节点后，所有成员随之更新。" /> : <div className="max-w-full min-w-0"><Table><TableHeader><TableRow><TableHead>策略组</TableHead><TableHead>可用节点与链路</TableHead><TableHead>已分配用户</TableHead><TableHead>操作</TableHead></TableRow></TableHeader><TableBody>{policies.data.map(p => <TableRow key={p.id}><TableCell><strong>{p.name}</strong>{p.legacy_group_id != null && <small className="text-muted-foreground mt-0.5 block text-xs">原权限组 #{p.legacy_group_id}{p.legacy_lines ? ` · 另有 ${p.legacy_lines} 条线路由原面板按此组提供` : ''}</small>}</TableCell><TableCell className="min-w-64 whitespace-normal"><ChipList empty="空组，不授予节点" items={[...p.node_ids.map(id => ({ label: nodeName(id) })), ...p.chain_ids.map(id => ({ label: chains.find(c => c.id === id)?.name ?? `链路 #${id}`, kind: 'chain' as const })), ...(p.external_nodes ?? []).map(entry => ({ label: entry.name, kind: 'external' as const }))]} /></TableCell><TableCell>{p.member_count}</TableCell><TableCell><div className="flex flex-wrap items-center justify-end gap-2"><Button variant="ghost" size="sm" disabled={action.busy || Boolean(policyWriteError)} onClick={() => open({ kind: 'policy-groups', value: p })}>编辑</Button><Button variant="ghost" size="sm" className="text-destructive hover:bg-destructive/10 hover:text-destructive dark:hover:bg-destructive/20" disabled={action.busy || Boolean(policyWriteError)} onClick={() => remove('policy-groups', p)}>删除</Button></div></TableCell></TableRow>)}</TableBody></Table></div>)}
      {tab === 'package-groups' && (packages.loading && !packages.data ? <Loading variant="table" rows={3} /> : !packages.data?.length ? <Empty icon={ActivityIcon} title="为不同用量创建套餐" description="例如每月 500 GiB、1 日零点重置、有效 365 天；每位用户单独计量。" /> : <div className="max-w-full min-w-0"><Table><TableHeader><TableRow><TableHead>套餐组</TableHead><TableHead>每月流量</TableHead><TableHead>重置时间</TableHead><TableHead>有效期</TableHead><TableHead>操作</TableHead></TableRow></TableHeader><TableBody>{packages.data.map(p => <TableRow key={p.id}><TableCell><strong>{p.name}</strong></TableCell><TableCell>{p.monthly_bytes === null ? '不限量' : bytes(p.monthly_bytes)}</TableCell><TableCell>{scheduleText(p)}</TableCell><TableCell>分配后 {p.duration_days} 天</TableCell><TableCell><div className="flex flex-wrap items-center justify-end gap-2"><Button variant="ghost" size="sm" disabled={action.busy || Boolean(packageWriteError)} onClick={() => open({ kind: 'package-groups', value: p })}>编辑</Button><Button variant="ghost" size="sm" className="text-destructive hover:bg-destructive/10 hover:text-destructive dark:hover:bg-destructive/20" disabled={action.busy || Boolean(packageWriteError)} onClick={() => remove('package-groups', p)}>删除</Button></div></TableCell></TableRow>)}</TableBody></Table></div>)}
      {tab === 'policy-groups' && <div className="border-t px-4 py-2"><a className={buttonVariants({ variant: 'ghost', size: 'sm' })} href="#/nodes?kind=chains">管理链路</a></div>}
    </Panel>
    {/* Assigning one group to many users at once belongs with the groups themselves. */}
    {tab === 'policy-groups' && users.data && users.data.length > 0 && <BatchProxyAssignments users={users.data} userError={() => resourceWriteError(users)} onChanged={refresh} />}
    <DialogPresence>{editor && <FormDialog title={`${'value' in editor && editor.value ? '编辑' : '创建'}${labels[editor.kind]}`} onClose={() => setEditor(null)} onSubmit={submit} busy={action.busy} disabled={Boolean(writeError(editor.kind, editor.value?.id))} submitDisabled={Boolean(selectionError)} error={writeError(editor.kind, editor.value?.id) || selectionError || action.error} retry={writeError(editor.kind, editor.value?.id) ? refresh : undefined}>
      <Field label="名称"><Input name="name" required maxLength={128} defaultValue={'value' in editor ? editor.value?.name ?? '' : ''} autoComplete="off" /></Field>
      {editor.kind === 'policy-groups' && <>
        <fieldset data-slot="group-choices" className="my-3 mb-4 grid gap-2.5 border-0 p-0"><legend className="mb-2.5 text-sm font-semibold">直接连接的节点</legend>{nodes.map(n => <label className="flex cursor-pointer items-center gap-3 py-2.5 text-sm" key={`direct:${n.id}`}><Checkbox name="node_ids" value={n.id} checked={editor.nodeIds.includes(n.id)} disabled={!n.available && !editor.nodeIds.includes(n.id) && !heldMigrated.includes(n.id)} onCheckedChange={checked => choose('nodeIds', n.id, checked === true)} /><span>{n.name}<small className="text-muted-foreground mt-0.5 block text-xs">{n.description}{!n.available && (heldMigrated.includes(n.id) ? '（迁移线路，所在主机目前由原面板运行）' : '（已不可用，请取消选择）')}</small></span></label>)}{invalidNodes.filter(id => !nodes.some(node => node.id === id)).map(id => <label className="flex cursor-pointer items-center gap-3 py-2.5 text-sm" key={`unavailable-${id}`}><Checkbox name="node_ids" value={id} checked onCheckedChange={checked => choose('nodeIds', id, checked === true)} /><span>{nodeName(id)}<small className="text-muted-foreground mt-0.5 block text-xs">当前不能作为普通节点授权，请取消选择。</small></span></label>)}{!nodes.length && <p>请先创建代理节点。</p>}</fieldset>
        <fieldset data-slot="group-choices" className="my-3 mb-4 grid gap-2.5 border-0 p-0"><legend className="mb-2.5 text-sm font-semibold">通过入口连接的链路</legend>{chains.map(c => <label className="flex cursor-pointer items-center gap-3 py-2.5 text-sm" key={`chain:${c.id}`}><Checkbox name="chain_ids" value={c.id} checked={editor.chainIds.includes(c.id)} onCheckedChange={checked => choose('chainIds', c.id, checked === true)} disabled={!c.available && !editor.chainIds.includes(c.id)} /><span>{c.name}<small className="text-muted-foreground mt-0.5 block text-xs">{c.description}{!c.available && '（已不可用，请取消选择）'}</small></span></label>)}{invalidChains.filter(id => !chains.some(chain => chain.id === id)).map(id => <label className="flex cursor-pointer items-center gap-3 py-2.5 text-sm" key={`unavailable-${id}`}><Checkbox name="chain_ids" value={id} checked onCheckedChange={checked => choose('chainIds', id, checked === true)} /><span>链路 #{id}<small className="text-muted-foreground mt-0.5 block text-xs">当前不可用，请取消选择。</small></span></label>)}{!chains.length && <p>尚未创建链路，可先只选择节点，或<a className={buttonVariants({ variant: 'ghost', size: 'sm' })} href="#/nodes?kind=chains">创建链路</a>。</p>}</fieldset>
        <GroupExternalNodes selected={editor.externals} held={editor.value?.external_nodes ?? []} candidates={candidates} disabled={action.busy} onChange={next => setEditor(previous => previous?.kind === 'policy-groups' ? { ...previous, externals: next } : previous)} />
        {editor.value?.legacy_group_id != null && <p className="text-muted-foreground mt-3 text-xs leading-relaxed">此组对应原权限组 #{editor.value.legacy_group_id}。迁移线路只对从原系统迁移的用户生效，按其原有身份提供；这些用户的订阅仍由原系统生成，只列出他们所属原权限组中的线路{editor.value.legacy_lines ? `，其中 ${editor.value.legacy_lines} 条仍由原面板运行` : ''}。</p>}
        {editor.value && <p className="text-muted-foreground mt-3 text-xs leading-relaxed">保存后会更新此组的 {editor.value.member_count} 位用户。新增授权等待设备应用，撤销授权同时移出订阅。</p>}
      </>}
      {editor.kind === 'package-groups' && <PackageFields value={editor.value} />}
    </FormDialog>}</DialogPresence>
    <DialogPresence>{deleting && <Confirm title={`删除「${deleting.name}」？`} busy={action.busy} disabled={Boolean(writeError(deleting.kind, deleting.id))} error={writeError(deleting.kind, deleting.id) || action.error} retry={writeError(deleting.kind, deleting.id) ? refresh : undefined} onClose={() => setDeleting(null)} onConfirm={() => { if (writeError(deleting.kind, deleting.id)) return; void action.run(() => api(`${root}/${deleting.kind}/${deleting.id}`, 'DELETE'), () => { setDeleting(null); refresh() }) }}>{deleting.kind === 'package-groups' ? '已分配用户的套餐与历史用量保持不变；此套餐不再提供新的分配。' : '已分配给用户的策略组不能直接删除，请先在用户页面取消分配。'}</Confirm>}</DialogPresence>
  </>
}

function PackageFields({ value }: { value?: PackageGroup }) {
  const gib = !value || (value.monthly_bytes !== null && BigInt(value.monthly_bytes) % 1073741824n === 0n)
  const amount = !value ? '500' : value.monthly_bytes === null ? '' : (BigInt(value.monthly_bytes) / (gib ? 1073741824n : 1n)).toString()
  return <>
    <div className="grid gap-4 sm:grid-cols-2"><Field label="每月流量" hint="留空表示不限量；填写正整数。"><Input name="amount" inputMode="numeric" pattern="[1-9][0-9]*" defaultValue={amount} /></Field><Field label="流量单位"><NativeSelect className="w-full" name="unit" defaultValue={gib ? 'GiB' : 'B'}><option value="GiB">GiB（1024³ 字节）</option><option value="B">字节</option></NativeSelect></Field></div>
    <div className="grid gap-4 sm:grid-cols-2"><Field label="每月重置日" hint="短月份自动取月末。"><Input name="reset_day" type="number" required min={1} max={31} step={1} defaultValue={value?.reset_day ?? 1} /></Field><Field label="重置时间"><Input name="reset_time" type="time" required defaultValue={`${String(value?.reset_hour ?? 0).padStart(2, '0')}:${String(value?.reset_minute ?? 0).padStart(2, '0')}`} /></Field></div>
    <Field label="重置时区"><TimezoneSelect name="timezone" required defaultValue={value?.timezone ?? 'Asia/Taipei'} /></Field>
    <Field label="可使用天数" hint="从分配时刻起计算，每天按 24 小时计。"><Input name="duration_days" type="number" required min={1} max={36500} step={1} defaultValue={value?.duration_days ?? 365} /></Field>
    <p className="text-muted-foreground mt-3 text-xs leading-relaxed">每月流量按用户所有节点的上下行合计；重新分配不清空本期流量，修改模板不影响已分配的套餐。</p>
  </>
}
