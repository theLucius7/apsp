import { useEffect, useRef, useState } from 'react'
import UserEntitlements from './UserEntitlements'
import ExternalUserAccess from './ExternalUserAccess'
import UserPasskeyAccess from './UserPasskeyAccess'
import UserDiagnostics from './UserDiagnostics'
import BatchProxyAssignments from './BatchProxyAssignments'
import SubscriptionDialog from './SubscriptionDialog'
import type { SubscriptionFormat } from './SubscriptionDialog'
import type { ProxyResource } from './resourceTypes'
import { api } from '../../api'
import { Badge, Confirm, Empty, ErrorNotice, Field, FormDialog, Loading, Modal, PageHeader, Refresh, Stat, DialogPresence } from '../../components'
import { bytes, totalBytes } from '../../format'
import { resourceWriteError, useAction, useResource } from '../../hooks'
import type { Access, Node, Usage, ProxyUser } from '../../types'
import type { Entitlement } from './groupTypes'
import { Button, buttonVariants } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Switch } from '@/components/ui/switch'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'
import { ArrowDownIcon, ArrowUpIcon, ChevronRightIcon, CopyIcon, PlusIcon, UsersIcon, WaypointsIcon } from 'lucide-react'
import { ListDetailLayout } from '@/components/list-detail'
import { Notice, Panel, PanelHeader, PanelTitle } from '@/components/panel'

export default function ProxyUsers() {
  const users = useResource<ProxyUser[]>('/api/plugins/sing-box/users')
  const nodes = useResource<Node[]>('/api/plugins/sing-box/nodes')
  const resources = useResource<ProxyResource[]>('/api/plugins/sing-box/proxy-resources')
  const usage = useResource<Usage>('/api/plugins/sing-box/usage')
  const action = useAction()
  const [selected, setSelected] = useState<number | null>(null)
  const selectedRef = useRef<number | null>(null)
  const selectUser = (id: number | null) => { selectedRef.current = id; setSelected(id) }
  const [editor, setEditor] = useState<ProxyUser | 'new' | null>(null)
  const [deleting, setDeleting] = useState<ProxyUser | null>(null)
  const [subscription, setSubscription] = useState<ProxyUser | null>(null)
  const [resetting, setResetting] = useState<ProxyUser | null>(null)
  const [format, setFormat] = useState<SubscriptionFormat>('singbox')
  const [notice, setNotice] = useState('')
  const [search, setSearch] = useState('')
  const [refreshRevision, setRefreshRevision] = useState(0)
  useEffect(() => {
    if (users.data && selectedRef.current === null) selectUser(users.data[0]?.id ?? null)
  }, [users.data])
  const accesses = useResource<Access[]>(selected ? `/api/plugins/sing-box/users/${selected}/accesses` : null)
  const entitlement = useResource<Entitlement>(selected ? `/api/plugins/sing-box/users/${selected}/entitlement` : null)
  const selectedUsage = useResource<Usage>(selected ? `/api/plugins/sing-box/usage?user_id=${selected}` : null)
  const user = users.data?.find(user => user.id === selected)
  const record = usage.data?.by_user.find(record => record.user_id === selected)
  const refresh = () => { users.reload(); nodes.reload(); resources.reload(); usage.reload(); accesses.reload(); entitlement.reload(); selectedUsage.reload(); setRefreshRevision(value => value + 1) }
  const userError = (id?: number) => {
    if (selected !== selectedRef.current) return '当前选择的代理用户已改变，请等待相关信息刷新后再操作；当前草稿已保留。'
    if (id !== undefined && users.isCurrent() && !users.getCurrent()?.some(value => value.id === id)) return '此代理用户已不存在，请重新选择；当前草稿已保留。'
    const stale = resourceWriteError(users, nodes, resources, ...(selected ? [accesses, entitlement] : []))
    if (stale) return stale
    if (id !== undefined && id !== selectedRef.current) return '当前选择的代理用户已改变，请关闭对话框后重新操作；当前草稿已保留。'
    return ''
  }
  const editorError = editor ? userError(editor === 'new' ? undefined : editor.id) : ''
  const deletingError = deleting ? userError(deleting.id) : ''
  const resettingError = resetting ? userError(resetting.id) : ''
  const edit = (value: ProxyUser | 'new') => { if (userError(value === 'new' ? undefined : value.id)) return; action.clearError(); setEditor(value) }
  const submit = (form: FormData) => {
    if (!editor || userError(editor === 'new' ? undefined : editor.id)) return
    void action.run(() => api<ProxyUser>(editor === 'new' ? '/api/plugins/sing-box/users' : `/api/plugins/sing-box/users/${editor?.id}`, editor === 'new' ? 'POST' : 'PATCH', { name: String(form.get('name')).trim() }), value => { setEditor(null); selectUser(value.id); users.reload() })
  }
  const grant = (node: Node, checked: boolean) => {
    if (!selected || userError(selected) || !nodes.getCurrent()?.some(value => value.id === node.id) || !resources.getCurrent()?.some(value => value.kind === 'direct' && value.id === node.id)) return
    setNotice('')
    const request = () => checked
      ? api(`/api/plugins/sing-box/users/${selected}/accesses`, 'POST', { node_id: node.id })
      : api(`/api/plugins/sing-box/users/${selected}/accesses/${node.id}`, 'DELETE')
    void action.run(request, () => {
      accesses.reload()
      setNotice(checked ? '授权已保存，设备应用新配置后会出现在订阅中。' : '授权已撤销，已从订阅移除；运行时将在新配置应用后更新。')
    })
  }
  const visible = users.data?.filter(user => user.name.toLocaleLowerCase().includes(search.toLocaleLowerCase())) ?? []
  const clearDeletedSelection = () => {
    if (users.isCurrent() && !users.getCurrent()?.some(value => value.id === selectedRef.current)) selectUser(null)
  }
  const remove = () => {
    if (!deleting || userError(deleting.id)) return
    void action.run(() => api(`/api/plugins/sing-box/users/${deleting.id}`, 'DELETE'), () => { setDeleting(null); selectUser(null); refresh() })
  }
  const reset = () => {
    if (!resetting || userError(resetting.id)) return
    void action.run(() => api<ProxyUser>(`/api/plugins/sing-box/users/${resetting.id}/subscription/reset`, 'POST'), value => {
      setResetting(null); setSubscription(value); users.reload(); setNotice('订阅链接已重置，请将新链接提供给代理用户。')
    })
  }
  const missingSelection = selected ? `代理用户 #${selected} 已不存在，请从列表重新选择。原选择没有自动切换。` : undefined
  const roster = users.data && <Panel>
    <PanelHeader><PanelTitle>代理用户列表 <span className="bg-muted text-muted-foreground min-w-5 rounded px-1.5 text-center text-xs font-medium tabular-nums">{users.data.length}</span></PanelTitle></PanelHeader>
    <div className="p-3"><Input aria-label="搜索代理用户" placeholder="搜索代理用户名称…" value={search} onChange={event => setSearch(event.target.value)} /></div>
    <div data-slot="user-roster" className="max-h-[580px] overflow-auto px-2 pb-2 max-md:flex max-md:max-h-none max-md:gap-1.5 max-md:px-3 max-md:pb-3">
      {visible.map(entry => {
        const total = usage.data?.by_user.find(record => record.user_id === entry.id)
        return <button key={entry.id} className="hover:bg-muted aria-pressed:bg-accent aria-pressed:text-accent-foreground focus-visible:ring-ring/50 mb-1 flex w-full items-center gap-3 rounded-lg px-3 py-2.5 text-left outline-none focus-visible:ring-3 disabled:opacity-60 max-md:mb-0 max-md:w-auto max-md:max-w-56 max-md:min-w-36 max-md:shrink-0 max-md:p-2.5" disabled={action.busy} aria-pressed={selected === entry.id}
          onClick={() => { selectUser(entry.id); setNotice(''); action.clearError() }}>
          <span className="bg-muted text-muted-foreground grid size-9 shrink-0 place-items-center rounded-lg border text-sm font-medium" aria-hidden="true">{entry.name.slice(0, 1)}</span>
          <span className="min-w-0 flex-1"><strong className="block truncate text-sm font-medium">{entry.name}</strong><small className="text-muted-foreground mt-0.5 block truncate text-xs">{total ? bytes(totalBytes(total.uplink, total.downlink)) : usage.data ? '尚无流量记录' : '流量加载中…'}</small></span>
          <ChevronRightIcon className="text-muted-foreground size-4 shrink-0 max-md:hidden" aria-hidden="true" />
        </button>
      })}
      {!visible.length && <p className="text-muted-foreground px-4 py-6 text-sm">没有匹配的代理用户。</p>}
    </div>
  </Panel>
  const grantableNodes = nodes.data?.filter(node => resources.data?.some(resource => resource.kind === 'direct' && resource.id === node.id)) ?? []
  const details = user && <>
    <Panel>
      <div data-slot="user-heading" className="flex items-center justify-between gap-4 p-4 max-md:flex-wrap">
        <div className="flex items-center gap-3 text-left"><span className="bg-muted text-muted-foreground grid shrink-0 place-items-center rounded-lg border font-medium size-11 text-lg">{user.name.slice(0, 1)}</span><div><h2 className="mb-0.5 text-lg font-semibold wrap-anywhere">{user.name}</h2><span className="text-muted-foreground text-xs">代理用户 #{user.id}</span></div></div>
        <div className="flex flex-wrap items-center justify-end gap-2">
          <Button variant="ghost" size="sm" disabled={Boolean(userError(user.id))} onClick={() => edit(user)}>编辑</Button>
          <Button variant="ghost" size="sm" className="text-destructive hover:bg-destructive/10 hover:text-destructive dark:hover:bg-destructive/20" disabled={Boolean(userError(user.id))} onClick={() => { if (userError(user.id)) return; action.clearError(); setDeleting(user) }}>删除</Button>
        </div>
      </div>
      <div className="grid grid-cols-3 px-4 pb-4">
        <div className="min-w-0 border-r px-4 first:pl-0 last:border-0"><span className="text-muted-foreground block text-xs">上传</span><strong className="mt-1.5 block text-xl font-semibold wrap-anywhere">{record ? bytes(record.uplink) : usage.data ? '0 B' : '暂无数据'}</strong></div>
        <div className="min-w-0 border-r px-4 first:pl-0 last:border-0"><span className="text-muted-foreground block text-xs">下载</span><strong className="mt-1.5 block text-xl font-semibold wrap-anywhere">{record ? bytes(record.downlink) : usage.data ? '0 B' : '暂无数据'}</strong></div>
        <div className="min-w-0 border-r px-4 first:pl-0 last:border-0"><span className="text-muted-foreground block text-xs">已授权节点</span><strong className="mt-1.5 block text-xl font-semibold wrap-anywhere">{accesses.data?.length ?? '—'}</strong></div>
      </div>
      <div className="bg-muted/40 flex items-center justify-between gap-5 border-t px-4 py-4 max-md:flex-wrap">
        <div><strong className="text-sm font-medium">代理用户专属订阅</strong><p className="text-muted-foreground mt-1 text-xs">仅包含已应用且健康的授权节点。</p></div>
        <Button size="sm" onClick={() => { setFormat('singbox'); setSubscription(user) }}><CopyIcon aria-hidden="true" />订阅链接</Button>
      </div>
    </Panel>
    <UserEntitlements key={user.id} id={user.id} entitlement={entitlement} userError={() => userError(user.id)} refreshRevision={refreshRevision} onChange={refresh} />
    <Panel>
      <PanelHeader><PanelTitle>单独节点授权</PanelTitle><span className="text-muted-foreground text-xs">更改后自动发布</span></PanelHeader>
      <div className="p-4">
        <ErrorNotice message={accesses.error || entitlement.error || userError(selected ?? undefined) || (!editor && !deleting && !resetting ? action.error : '')} retry={accesses.error ? accesses.reload : undefined} />
        {notice && <Notice tone="success" role="status">{notice}</Notice>}
        {accesses.loading && !accesses.data ? <Loading variant="table" rows={3} /> : !nodes.data?.length
          ? <Empty icon={WaypointsIcon} title="还没有可授权的节点" description="先创建节点，再为代理用户开启访问权限。"><a className={buttonVariants({ variant: 'outline' })} href="#/plugins/sing-box/nodes">前往节点</a></Empty>
          : <div className="grid gap-2.5">{grantableNodes.map(node => {
            const access = accesses.data?.find(access => access.node_id === node.id)
            return <label className="hover:bg-muted/40 relative flex cursor-pointer items-center gap-3 rounded-lg border p-3" key={node.id}>
              <span className="bg-muted text-muted-foreground grid size-9 shrink-0 place-items-center rounded-lg border"><WaypointsIcon className="size-4" aria-hidden="true" /></span>
              <span className="min-w-0 flex-1"><strong className="block text-sm font-medium wrap-anywhere">{node.name}</strong><small className="text-muted-foreground mt-0.5 block text-xs wrap-anywhere">{node.public_host}:{node.port}</small></span>
              {access && <Badge tone="good">{access.direct_grant ? '单独授权' : '来自策略组'}</Badge>}
              <Switch checked={Boolean(access?.direct_grant)} disabled={action.busy || Boolean(userError(user.id))} onCheckedChange={checked => grant(node, checked)} aria-label={`授权 ${node.name}`} />
            </label>
          })}</div>}
        <p className="text-muted-foreground mt-3 text-xs leading-relaxed">此开关只管理单独授权，不取消策略组授予的权限。只有全部授权来源移除后才撤销凭据；再次授权需要更新订阅。链路只能通过策略组分配。设备离线时等待重连应用。</p>
      </div>
    </Panel>
    <Panel>
      <PanelHeader><PanelTitle>该代理用户的节点流量</PanelTitle><span className="text-muted-foreground text-xs">包含历史记录</span></PanelHeader>
      <ErrorNotice message={selectedUsage.error} retry={selectedUsage.reload} />
      {selectedUsage.loading && !selectedUsage.data ? <Loading /> : selectedUsage.data?.by_node.length
        ? <div className="max-w-full min-w-0"><Table>
          <TableHeader><TableRow><TableHead>节点</TableHead><TableHead>上传</TableHead><TableHead>下载</TableHead><TableHead>合计</TableHead></TableRow></TableHeader>
          <TableBody>{selectedUsage.data.by_node.map(record => <TableRow key={record.node_id}>
            <TableCell>{record.name}{record.deleted && <span className="bg-muted text-muted-foreground ml-2 inline-block rounded px-1.5 py-0.5 align-middle text-xs">已删除</span>}</TableCell>
            <TableCell>{bytes(record.uplink)}</TableCell><TableCell>{bytes(record.downlink)}</TableCell><TableCell>{bytes(totalBytes(record.uplink, record.downlink))}</TableCell>
          </TableRow>)}</TableBody>
        </Table></div>
        : <div className="text-muted-foreground px-4 py-6 text-sm">此代理用户尚无节点流量记录。使用代理后，通常在 1 至 2 分钟内更新。</div>}
    </Panel>
  </>
  return <>
    <PageHeader eyebrow="代理服务" title="代理用户" description="为代理用户分配策略组（能用哪些节点）和套餐（流量与有效期），管理订阅链接并查看实际代理流量。">
      <Refresh onClick={refresh} />
      <Button disabled={Boolean(userError())} onClick={() => edit('new')}><PlusIcon aria-hidden="true" />创建代理用户</Button>
    </PageHeader>
    <div className="mb-6 grid grid-cols-3 gap-3 max-sm:gap-2">
      <Stat icon={UsersIcon} label="代理用户总数" value={users.data?.length ?? '—'} note="每个代理用户拥有独立订阅" />
      <Stat icon={ArrowUpIcon} label="累计上传" value={usage.data ? bytes(usage.data.uplink) : '—'} note="认证端实际统计的上传流量" />
      <Stat icon={ArrowDownIcon} label="累计下载" value={usage.data ? bytes(usage.data.downlink) : '—'} note="含已删除代理用户的历史用量" />
    </div>
    {users.data?.length ? <BatchProxyAssignments users={users.data} userError={() => userError()} onChanged={refresh} /> : null}
    <ErrorNotice message={users.error || nodes.error || resources.error || usage.error} retry={refresh} />
    <ErrorNotice message={users.ready && selected && !user ? missingSelection : undefined} />
    {users.ready && selected && !user && <Button variant="outline" size="sm" disabled={action.busy} onClick={clearDeletedSelection}>清除已删除的用户选择</Button>}
    {users.loading && !users.data ? <Panel><Loading variant="table" /></Panel> : !users.data?.length
      ? <Panel><Empty icon={UsersIcon} title="创建代理用户，开始分配节点" description="每个代理用户在每个节点上的凭据相互独立，流量按代理用户和节点统计。">
        <Button disabled={Boolean(userError())} onClick={() => edit('new')}><PlusIcon aria-hidden="true" />创建代理用户</Button>
      </Empty></Panel>
      : <ListDetailLayout id="apsp-proxy-users" label="调整代理用户列表宽度" list={roster} detail={<div className="min-w-0">
          {details || <Panel><div className="p-4"><ErrorNotice message={missingSelection} /></div></Panel>}
        </div>} />}
    {/* Kept outside the list layout: retained drafts must survive an emptied or failed user list. */}
    {selected !== null && <UserPasskeyAccess key={`portal-${selected}`} id={selected} userError={() => userError(selected)} refreshRevision={refreshRevision} />}
    {selected !== null && <UserDiagnostics key={`diagnosis-${selected}`} userId={selected} userError={() => userError(selected)} onChanged={refresh} />}
    {selected !== null && <ExternalUserAccess key="external-selected" userId={selected} userError={() => userError(selected)} refreshRevision={refreshRevision} onChanged={refresh} />}
    {usage.data?.by_user.some(record => record.deleted) && <Panel>
      <PanelHeader><PanelTitle>历史代理用户流量</PanelTitle><span className="text-muted-foreground text-xs">已删除代理用户的数据仍保留</span></PanelHeader>
      <div className="max-w-full min-w-0"><Table>
        <TableHeader><TableRow><TableHead>代理用户</TableHead><TableHead>上传</TableHead><TableHead>下载</TableHead></TableRow></TableHeader>
        <TableBody>{usage.data.by_user.filter(record => record.deleted).map(record => <TableRow key={record.user_id}>
          <TableCell>{record.name} <span className="bg-muted text-muted-foreground ml-2 inline-block rounded px-1.5 py-0.5 align-middle text-xs">已删除</span></TableCell><TableCell>{bytes(record.uplink)}</TableCell><TableCell>{bytes(record.downlink)}</TableCell>
        </TableRow>)}</TableBody>
      </Table></div>
    </Panel>}
    <DialogPresence>{editor && <FormDialog title={editor === 'new' ? '创建代理用户' : '编辑代理用户'} onClose={() => setEditor(null)} onSubmit={submit} busy={action.busy}
      submitDisabled={Boolean(editorError)} error={editorError || action.error} submitLabel={editor === 'new' ? '创建代理用户' : '保存修改'}>
      <Field label="代理用户名称"><Input name="name" required maxLength={128} defaultValue={editor === 'new' ? '' : editor.name} placeholder="为使用者设置一个名称" autoComplete="off" /></Field>
    </FormDialog>}</DialogPresence>
    <DialogPresence>{deleting && <Confirm title={`删除「${deleting.name}」？`} busy={action.busy} confirmDisabled={Boolean(deletingError)} error={deletingError || action.error}
      onClose={() => setDeleting(null)} onConfirm={remove}>
      此代理用户的订阅链接将失效，全部节点授权会被撤销。历史用量保留，受管设备应用新配置后停止旧授权；已下载的外部凭据由提供方控制。
    </Confirm>}</DialogPresence>
    <DialogPresence>{resetting && <Modal title={`重置「${resetting.name}」的订阅链接？`} busy={action.busy} onClose={() => setResetting(null)}>
      <div data-slot="modal-body" className="p-4">
        <ErrorNotice message={resettingError || action.error} />
        <p className="text-muted-foreground text-sm leading-relaxed">旧链接将立即失效，代理用户需要在客户端换成新链接。现有节点连接凭据和授权保持不变，已下载的配置仍可使用。</p>
      </div>
      <footer>
        <Button variant="outline" disabled={action.busy} onClick={() => setResetting(null)}>取消</Button>
        <Button variant="destructive" disabled={action.busy || Boolean(resettingError)} onClick={reset}>{action.busy ? '正在重置…' : '确认重置'}</Button>
      </footer>
    </Modal>}</DialogPresence>
    <DialogPresence>{subscription && <SubscriptionDialog user={subscription} format={format} onFormatChange={setFormat} onClose={() => setSubscription(null)}
      onReset={() => { if (userError(subscription.id)) return; action.clearError(); setResetting(subscription); setSubscription(null) }} />}</DialogPresence>
  </>
}
