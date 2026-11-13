import UserSessions from './UserSessions'
import UserControl from './UserControl'
import { useEffect, useRef, useState } from 'react'
import UserEntitlements from './UserEntitlements'
import UserPasskeyAccess from './UserPasskeyAccess'
import UserDiagnostics from './UserDiagnostics'
import UserLimits from './UserLimits'
import SubscriptionDialog from './SubscriptionDialog'
import PortalSettings from './accounts/PortalSettings'
import UserInvitations from './accounts/UserInvitations'
import type { SubscriptionFormat } from './SubscriptionDialog'
import { api, errorMessage } from '../api'
import { Badge, Confirm, Empty, ErrorNotice, Field, FormDialog, Loading, Modal, PageHeader, Refresh, DialogPresence, InfoTip } from '../components'
import { bytes, totalBytes } from '../format'
import { resourceWriteError, useAction, useResource } from '../hooks'
import type { Usage, ProxyUser } from '../types'
import { statusText } from './groupTypes'
import type { Entitlement } from './groupTypes'
import { copyText } from '@/lib/clipboard'
import { cn } from '@/lib/utils'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'
import { CheckIcon, CopyIcon, PlusIcon, Settings2Icon, UsersIcon } from 'lucide-react'
import { Notice, Panel, PanelHeader, PanelTitle } from '@/components/panel'
import { SegmentedButton, SegmentedNav } from '@/components/segmented'
import { useActor } from '@/lib/actor'

/** A line of the user list from /users/overview; contains no credentials. */
type UserLine = {
  id: number; status: Entitlement['status']; allowed: boolean; package_name: string | null; monthly_bytes: string | null; used_bytes: string
  expires_at: number | null; group_ids: number[]; external_nodes: number; bridged?: boolean; portal_created: boolean; portal_keys: number; activation_expires_at: number | null
}
type Section = 'usage' | 'invitations' | 'diagnosis'
const sections: readonly { key: Section; label: string }[] = [{ key: 'usage', label: '流量明细' }, { key: 'invitations', label: '邀请' }, { key: 'diagnosis', label: '诊断与安全' }]
const root = '/api/access'

const lineTone = (line?: UserLine) => !line ? 'bg-muted-foreground/40' : !line.allowed ? 'bg-destructive' : line.status === 'not_started' ? 'bg-info' : quotaUsed(line) >= 90 ? 'bg-warning' : line.status === 'active' ? 'bg-success' : 'bg-muted-foreground/40'
const quotaUsed = (line: UserLine) => line.monthly_bytes ? Number(line.used_bytes) / Number(line.monthly_bytes) * 100 : 0

/** The list answers "who is this and can they use it" without opening anyone. */
function lineSummary(line?: UserLine) {
  if (!line) return ''
  if (line.status === 'paused' || line.status === 'expired' || line.status === 'exhausted' || line.status === 'not_started') return statusText[line.status]
  if (!line.package_name) return '未设套餐'
  return `${line.package_name} · ${bytes(line.used_bytes)} / ${line.monthly_bytes ? bytes(line.monthly_bytes) : '不限量'}`
}

/** `initialUserId` opens one user, as the subscription board's cards do. */
export default function ProxyUsers({ initialUserId }: { initialUserId?: number }) {
  const users = useResource<ProxyUser[]>(`${root}/users`, 5000, { keep: true })
  const lines = useResource<UserLine[]>(`${root}/users/overview`, 15_000, { keep: true })
  const usage = useResource<Usage>(`${root}/usage`, 5000, { keep: true })
  const actor = useActor()
  const action = useAction()
  const linkAction = useAction()
  const [selected, setSelected] = useState<number | null>(initialUserId ?? null)
  const selectedRef = useRef<number | null>(initialUserId ?? null)
  const selectUser = (id: number | null) => { selectedRef.current = id; setSelected(id) }
  const [editor, setEditor] = useState<ProxyUser | 'new' | null>(null)
  const [deleting, setDeleting] = useState<ProxyUser | null>(null)
  const [subscription, setSubscription] = useState<ProxyUser | null>(null)
  const [portalOpen, setPortalOpen] = useState(false)
  const [resetting, setResetting] = useState<ProxyUser | null>(null)
  const [format, setFormat] = useState<SubscriptionFormat>('singbox')
  const [notice, setNotice] = useState('')
  const [search, setSearch] = useState('')
  const [section, setSection] = useState<Section>('usage')
  const [refreshRevision, setRefreshRevision] = useState(0)
  // Addresses stay hidden until the administrator proved themselves recently; what
  // a reveal returned is kept for this page so copying again asks nothing more.
  const [revealed, setRevealed] = useState<Record<number, string>>({})
  const [copied, setCopied] = useState<number | null>(null)
  useEffect(() => { if (copied === null) return; const timer = window.setTimeout(() => setCopied(null), 2000); return () => window.clearTimeout(timer) }, [copied])
  useEffect(() => {
    if (users.data && selectedRef.current === null) selectUser(users.data[0]?.id ?? null)
  }, [users.data])
  // A user opened by address is scrolled into view in the roster once it is drawn.
  const rosterReady = Boolean(users.data)
  useEffect(() => {
    if (initialUserId && rosterReady) document.querySelector(`[data-slot=user-roster] [data-user-id="${initialUserId}"]`)?.scrollIntoView({ block: 'nearest' })
  }, [initialUserId, rosterReady])
  const entitlement = useResource<Entitlement>(selected ? `${root}/users/${selected}/entitlement` : null)
  const selectedUsage = useResource<Usage>(selected ? `${root}/usage?user_id=${selected}` : null)
  const user = users.data?.find(user => user.id === selected)
  const line = lines.data?.find(line => line.id === selected)
  const record = usage.data?.by_user.find(record => record.user_id === selected)
  const refresh = () => { users.reload(); lines.reload(); usage.reload(); entitlement.reload(); selectedUsage.reload(); setRefreshRevision(value => value + 1) }
  const userError = (id?: number) => {
    if (selected !== selectedRef.current) return '当前选择的代理用户已改变，请等待相关信息刷新后再操作；当前草稿已保留。'
    if (id !== undefined && users.isCurrent() && !users.getCurrent()?.some(value => value.id === id)) return '此代理用户已不存在，请重新选择；当前草稿已保留。'
    const stale = resourceWriteError(users, ...(selected ? [entitlement] : []))
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
    void action.run(() => api<ProxyUser>(editor === 'new' ? `${root}/users` : `${root}/users/${editor?.id}`, editor === 'new' ? 'POST' : 'PATCH', { name: String(form.get('name')).trim() }), value => { setEditor(null); selectUser(value.id); users.reload(); lines.reload() })
  }
  const visible = users.data?.filter(user => user.name.toLocaleLowerCase().includes(search.toLocaleLowerCase())) ?? []
  const clearDeletedSelection = () => {
    if (users.isCurrent() && !users.getCurrent()?.some(value => value.id === selectedRef.current)) selectUser(null)
  }
  const remove = () => {
    if (!deleting || userError(deleting.id)) return
    void action.run(() => api(`${root}/users/${deleting.id}`, 'DELETE'), () => { setDeleting(null); selectUser(null); refresh() })
  }
  const reset = () => {
    if (!resetting || userError(resetting.id)) return
    void action.run(() => api<ProxyUser>(`${root}/users/${resetting.id}/subscription/reset`, 'POST'), value => {
      setResetting(null); setSubscription(value); setRevealed(current => ({ ...current, [value.id]: value.subscription_url })); users.reload(); setNotice('订阅链接已重置，请将新链接提供给代理用户。')
    })
  }
  // One action from anywhere in the list: asks for the recent proof only when the
  // panel requires it, then the address is on the clipboard.
  const copySubscription = (target: ProxyUser) => void linkAction.run(async () => {
    let address = target.subscription_url || revealed[target.id]
    if (!address) {
      address = (await api<{ subscription_url: string }>(`${root}/users/${target.id}/subscription?format=singbox`)).subscription_url
      setRevealed(current => ({ ...current, [target.id]: address }))
      users.reload()
    }
    // A browser may refuse after the wait for proof: the address is on screen by then.
    try { await copyText(address) } catch (reason) { throw new Error(`复制失败，请选中订阅链接手动复制。${errorMessage(reason)}`) }
    setCopied(target.id)
  })
  const missingSelection = selected ? `代理用户 #${selected} 已不存在，请从列表重新选择。原选择没有自动切换。` : undefined
  const address = user ? user.subscription_url || revealed[user.id] || '' : ''
  const empty = Boolean(users.data && !users.data.length)
  return <>
    <PageHeader title="用户">
      <Refresh onClick={refresh} />
      <Button variant="outline" onClick={() => setPortalOpen(true)}><Settings2Icon aria-hidden="true" />门户设置</Button>
      <Button disabled={Boolean(userError())} onClick={() => edit('new')}><PlusIcon aria-hidden="true" />创建代理用户</Button>
    </PageHeader>
    <ErrorNotice message={users.error || usage.error} retry={refresh} />
    <ErrorNotice message={linkAction.error} retry={linkAction.clearError} />
    {users.ready && selected && !user && <div className="mb-4 flex flex-wrap items-center gap-2"><ErrorNotice message={missingSelection} /><Button variant="outline" size="sm" disabled={action.busy} onClick={clearDeletedSelection}>清除已删除的用户选择</Button></div>}
    {/* One tree at every width and for every list state: retained drafts and open dialogs survive a failed or emptied list. */}
    <div data-slot="list-detail" className="grid min-w-0 items-start gap-4 md:grid-cols-[17.5rem_minmax(0,1fr)]">
      <Panel className={cn('mb-0 md:sticky md:top-[4.25rem]', empty && selected === null && 'md:col-span-2')}>
        {users.loading && !users.data ? <Loading variant="table" /> : empty ? <Empty icon={UsersIcon} title="创建代理用户，开始分配节点" description="每个代理用户在每个节点上的凭据相互独立，流量按代理用户和节点统计。">
          <Button disabled={Boolean(userError())} onClick={() => edit('new')}><PlusIcon aria-hidden="true" />创建代理用户</Button>
        </Empty> : users.data && <>
          <div className="border-b p-2"><Input aria-label="搜索代理用户" placeholder={`搜索 ${users.data.length} 个代理用户…`} value={search} onChange={event => setSearch(event.target.value)} /></div>
          <div data-slot="user-roster" className="scrollbar-thin max-h-[calc(100dvh-13rem)] overflow-auto p-1 max-md:max-h-64">
            {visible.map(entry => {
              const entryLine = lines.data?.find(value => value.id === entry.id)
              const total = usage.data?.by_user.find(record => record.user_id === entry.id)
              const active = selected === entry.id
              return <div key={entry.id} data-user-id={entry.id} className={cn('hover:bg-muted flex items-center rounded-md', active && 'bg-accent text-accent-foreground hover:bg-accent')}>
                <button type="button" className="focus-visible:ring-ring/50 flex min-w-0 flex-1 items-center gap-2.5 rounded-md px-2 py-1.5 text-left outline-none focus-visible:ring-3 disabled:opacity-60" disabled={action.busy} aria-pressed={active}
                  onClick={() => { selectUser(entry.id); setNotice(''); action.clearError(); linkAction.clearError() }}>
                  <span title={entryLine ? statusText[entryLine.status] : undefined} className={cn('size-2 shrink-0 rounded-full', lineTone(entryLine))} aria-hidden="true" />
                  <span className="min-w-0 flex-1"><strong className="block truncate text-[13px] font-medium">{entry.name}</strong><small className={cn('text-muted-foreground block truncate text-xs', active && 'text-accent-foreground/75')}>{lineSummary(entryLine) || (total ? bytes(totalBytes(total.uplink, total.downlink)) : usage.data ? '尚无流量记录' : '')}</small></span>
                </button>
              </div>
            })}
            {!visible.length && <p className="text-muted-foreground px-3 py-5 text-[13px]">没有匹配的代理用户。</p>}
          </div>
        </>}
      </Panel>
      {selected !== null && <div className="min-w-0">
        {user && <div data-slot="user-heading" className="mb-3 flex flex-wrap items-center gap-x-3 gap-y-1">
          <h2 className="text-sm font-semibold wrap-anywhere">{user.name}</h2>
          {line && <Badge tone={!line.allowed ? 'bad' : line.status === 'active' ? 'good' : 'neutral'}>{line.status === 'unmetered' ? '不限流量' : statusText[line.status]}</Badge>}
          <span className="text-muted-foreground text-xs tabular-nums">#{user.id}</span>
          <div className="ml-auto flex items-center gap-1">
            <Button variant="ghost" size="sm" disabled={Boolean(userError(user.id))} onClick={() => edit(user)}>编辑</Button>
            <Button variant="ghost" size="sm" className="text-destructive hover:bg-destructive/10 hover:text-destructive dark:hover:bg-destructive/20" disabled={Boolean(userError(user.id))} onClick={() => { if (userError(user.id)) return; action.clearError(); setDeleting(user) }}>删除</Button>
          </div>
        </div>}
        {!user && !users.ready && !users.data?.length && users.error ? null : !user && <Panel><div className="p-4"><ErrorNotice message={missingSelection} /></div></Panel>}
        {/* How this user connects and signs in, side by side in one card. */}
        <Panel data-slot="user-access" className="mb-4 grid divide-y lg:grid-cols-2 lg:divide-x lg:divide-y-0">
          {user ? <section aria-label="订阅链接" className="flex min-w-0 flex-col gap-2.5 p-4">
            <div className="flex min-h-7 items-center justify-between gap-2">
              <span className="flex min-w-0 items-center gap-1.5"><h3 className="text-sm font-semibold">订阅链接</h3><InfoTip label="订阅链接说明"><p>sing-box 和 Clash 用同一个地址，客户端会自动拿到对应格式。重置后旧专属地址立即失效。</p></InfoTip></span>
              <Button variant="ghost" size="xs" onClick={() => { linkAction.clearError(); setFormat('singbox'); setSubscription(user) }}>订阅详情</Button>
            </div>
            {address ? <code data-slot="subscription-address" tabIndex={0} className="bg-muted/50 focus-visible:ring-ring/50 scrollbar-thin block overflow-x-auto rounded-md border px-2.5 py-2 text-xs break-normal whitespace-pre focus-visible:ring-3 focus-visible:outline-none">{address}</code>
              : <p className="bg-muted/50 text-muted-foreground rounded-md border px-2.5 py-2 text-xs">链接包含连接凭据，复制时会先确认管理员身份。</p>}
            <div className="mt-auto flex flex-wrap items-center gap-2">
              <Button size="sm" disabled={linkAction.busy} onClick={() => copySubscription(user)}>{copied === user.id ? <CheckIcon aria-hidden="true" /> : <CopyIcon aria-hidden="true" />}{copied === user.id ? '已复制' : linkAction.busy ? '正在获取…' : '复制订阅链接'}</Button>
            </div>
          </section> : <div />}
          <UserPasskeyAccess key={`portal-${selected}`} id={selected} userError={() => userError(selected)} refreshRevision={refreshRevision} />
        </Panel>
        {line?.bridged && <Notice role="status">迁移用户：订阅仍由原系统生成，线路来自所属「原权限组」，修改套餐不改变线路。</Notice>}
        {notice && <Notice tone="success" role="status">{notice}</Notice>}
        <ErrorNotice message={!editor && !deleting && !resetting ? action.error : ''} />
        {user && <UserEntitlements key={user.id} id={user.id} entitlement={entitlement} userError={() => userError(user.id)} refreshRevision={refreshRevision} onChange={refresh} bridged={Boolean(line?.bridged)} />}
        {user && <UserLimits key={`limits-${user.id}`} id={user.id} userError={() => userError(user.id)} refreshRevision={refreshRevision} />}
        <SegmentedNav aria-label="更多用户信息" className="mb-3">
          {sections.map(entry => <SegmentedButton key={entry.key} aria-pressed={section === entry.key} onClick={() => setSection(entry.key)}>{entry.label}</SegmentedButton>)}
        </SegmentedNav>
        {/* Hidden, not removed: a draft in one section survives looking at another. */}
        <div hidden={section !== 'usage'}>
          {user && <Panel>
            <PanelHeader className="min-h-10 py-1.5"><PanelTitle>该代理用户的节点流量</PanelTitle><span className="text-muted-foreground text-xs tabular-nums">↑ {record ? bytes(record.uplink) : '0 B'} · ↓ {record ? bytes(record.downlink) : '0 B'}，含历史记录</span></PanelHeader>
            <ErrorNotice message={selectedUsage.error} retry={selectedUsage.reload} />
            {selectedUsage.loading && !selectedUsage.data ? <Loading /> : selectedUsage.data?.by_node.length
              ? <div className="max-w-full min-w-0"><Table>
                <TableHeader><TableRow><TableHead>节点</TableHead><TableHead>上传</TableHead><TableHead>下载</TableHead><TableHead>合计</TableHead></TableRow></TableHeader>
                <TableBody>{selectedUsage.data.by_node.map(record => <TableRow key={record.node_id}>
                  <TableCell>{record.name}{record.deleted && <span className="bg-muted text-muted-foreground ml-2 inline-block rounded px-1.5 py-0.5 align-middle text-xs">已删除</span>}</TableCell>
                  <TableCell>{bytes(record.uplink)}</TableCell><TableCell>{bytes(record.downlink)}</TableCell><TableCell>{bytes(totalBytes(record.uplink, record.downlink))}</TableCell>
                </TableRow>)}</TableBody>
              </Table></div>
              : <div className="text-muted-foreground px-4 py-6 text-[13px]">此代理用户尚无节点流量记录。使用代理后，通常在 1 至 2 分钟内更新。</div>}
          </Panel>}
        </div>
        <div hidden={section !== 'invitations'}>
          <UserInvitations key={`invitations-${selected}`} userId={selected} active={section === 'invitations'} />
        </div>
        <div hidden={section !== 'diagnosis'}>
          <UserSessions key={`sessions-${selected}`} userId={selected} active={section === 'diagnosis'} />
          <UserControl key={`control-${selected}`} userId={selected} active={section === 'diagnosis'} onChanged={refresh} />
          <UserDiagnostics key={`diagnosis-${selected}`} userId={selected} active={section === 'diagnosis'} userError={() => userError(selected)} onChanged={refresh} />
        </div>
      </div>}
    </div>
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
        <p className="text-muted-foreground text-sm leading-relaxed">旧链接立即失效，用户需要在客户端换成新链接；节点凭据和授权不变。</p>
      </div>
      <footer>
        <Button variant="outline" disabled={action.busy} onClick={() => setResetting(null)}>取消</Button>
        <Button variant="destructive" disabled={action.busy || Boolean(resettingError)} onClick={reset}>{action.busy ? '正在重置…' : '确认重置'}</Button>
      </footer>
    </Modal>}</DialogPresence>
    <DialogPresence>{portalOpen && <PortalSettings owner={actor.data?.role === 'owner'} onClose={() => setPortalOpen(false)} onDomainSaved={() => { setRevealed({}); refresh() }} />}</DialogPresence>
    <DialogPresence>{subscription && <SubscriptionDialog user={subscription} format={format} onFormatChange={setFormat} onClose={() => setSubscription(null)}
      onReset={() => { if (userError(subscription.id)) return; action.clearError(); setResetting(subscription); setSubscription(null) }} />}</DialogPresence>
  </>
}
