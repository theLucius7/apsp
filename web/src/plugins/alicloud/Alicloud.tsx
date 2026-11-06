import { useState } from 'react'
import { api } from '../../api'
import { Confirm, Empty, ErrorNotice, Loading, PageHeader, Refresh, DialogPresence } from '../../components'
import { useAction, useResource } from '../../hooks'
import { AccountPanel } from './Accounts'
import type { CloudActions } from './Accounts'
import { Activity } from './Activity'
import { AccountEditor, BandwidthEditor, ResourceEditor } from './Editors'
import { EipRotationEditor } from './EipRotation'
import { PowerEditor, PowerPolicyEditor } from './Power'
import type { Account, Overview, Resource } from './types'
import { accountError, bandwidthError, checked, cloudError, powerError, resourceError } from './guards'

import { Button } from '@/components/ui/button'
import { CloudIcon, PlusIcon } from 'lucide-react'
import { Panel } from '@/components/panel'

type Editor = { type: 'account'; value?: Account } | { type: 'resource'; value?: Resource; accountId?: string } | { type: 'bandwidth'; value: Resource } | { type: 'power-policy'; value: Resource } | { type: 'power'; value: Resource; action: 'start' | 'stop' } | { type: 'eip-rotation'; value: Resource }

/**
 * Alibaba Cloud CDT, ECS power and public bandwidth. Each account is one card
 * with its CDT pools and its registered resources nested below; everything
 * that happened (bandwidth changes, power jobs, public IP rotations, notices)
 * is one history.
 * Every write still goes through the guarded dialogs and revision checks.
 */
export default function Alicloud() {
  const [editing, setEditing] = useState<Editor | null>(null), [removing, setRemoving] = useState<{ type: 'accounts'; value: Account } | { type: 'resources'; value: Resource; accountRevision: number | undefined } | null>(null)
  const [resume, setResume] = useState<{ value: Resource; accountRevision: number | undefined } | null>(null)
  const overview = useResource<Overview>('/api/plugins/alicloud'), action = useAction()
  const data = overview.data, available = Boolean(data && overview.ready && !overview.error)
  const done = () => { setEditing(null); setRemoving(null); setResume(null); overview.reload() }
  const current = overview.getCurrent
  const perform = (error: () => string, request: () => Promise<unknown>, success = overview.reload) => void action.run(() => checked(error, request), success)
  const open = (editor: Editor) => {
    const reason = editor.type === 'account' ? accountError(current, editor.value)
      : editor.type === 'resource' ? editor.value ? resourceError(current, editor.value) : cloudError(current)
        : editor.type === 'bandwidth' ? bandwidthError(current, editor.value)
          : editor.type === 'eip-rotation' ? resourceError(current, editor.value, { managed: true, idle: true }) : resourceError(current, editor.value, { managed: true, idle: true, power: true })
    if (reason) return
    action.clearError(); setEditing(editor)
  }
  const removalError = () => !removing ? '' : removing.type === 'accounts'
    ? accountError(current, removing.value) || (current()?.resources.some(r => r.account_id === removing.value.id) ? '请先移除此账号的全部资源登记。' : '')
    : resourceError(current, removing.value, { idle: true, accountRevision: removing.accountRevision })
  const resumeError = () => !resume ? '' : powerError(current, resume.value, undefined, resume.accountRevision) ||
    (!current()?.resources.find(r => r.id === resume.value.id)?.manual_hold ? '手动暂停状态已变化，请重新核对。' : '')
  const actions: CloudActions = {
    available, busy: action.busy, full: (data?.resources.length ?? 0) >= 32,
    refreshAccount: account => perform(() => accountError(current, account, true), () => api(`/api/plugins/alicloud/accounts/${account.id}/refresh`, 'POST')),
    editAccount: account => open({ type: 'account', value: account }),
    addResource: account => open({ type: 'resource', accountId: account.id }),
    removeAccount: account => { action.clearError(); setRemoving({ type: 'accounts', value: account }) },
    refreshResource: (resource, account) => perform(() => resourceError(current, resource, { managed: true, accountRevision: account?.revision }), () => api(`/api/plugins/alicloud/resources/${resource.id}/refresh`, 'POST')),
    editResource: resource => open({ type: 'resource', value: resource }),
    bandwidth: resource => open({ type: 'bandwidth', value: resource }),
    powerPolicy: resource => open({ type: 'power-policy', value: resource }),
    power: (resource, verb) => open({ type: 'power', value: resource, action: verb }),
    rotateEip: resource => open({ type: 'eip-rotation', value: resource }),
    resume: (resource, account) => { action.clearError(); setResume({ value: resource, accountRevision: account?.revision }) },
    removeResource: (resource, account) => { action.clearError(); setRemoving({ type: 'resources', value: resource, accountRevision: account?.revision }) },
  }
  const orphans = data?.resources.filter(resource => !data.accounts.some(account => account.id === resource.account_id)) ?? []
  return <div className="grid min-w-0 gap-5">
    <div className="grid gap-2">
      <PageHeader title="阿里云 CDT 与带宽"><Refresh onClick={overview.reload} /><Button disabled={!available || action.busy || (data?.accounts.length ?? 8) >= 8} onClick={() => open({ type: 'account' })}><PlusIcon aria-hidden="true" />添加云账号</Button></PageHeader>
      <details data-slot="cloud-reading-note" className="text-muted-foreground text-xs leading-relaxed">
        <summary className="w-fit">用量与费用读数说明</summary>
        <p className="mt-2 max-w-3xl">中国内地 / 海外 CDT 用量是兼容 CDT-Monitor 的读数（1 GiB = 1024³ 字节），只用于展示；CDT 守护每分钟单独读取所属额度池，自动降速依据带账期的完整 CDT 账单，账单有延迟。查询失败时保留上次读数，以“更新于”为准。账单与余额是可选查询，在账号设置中开启，缓存 6 小时，以云端为准。免费额度与开通状态请在<a className="text-primary underline-offset-4 hover:underline" href="https://cdt.console.aliyun.com/" target="_blank" rel="noreferrer">阿里云 CDT 控制台</a>核对。</p>
      </details>
    </div>
    <ErrorNotice message={overview.error || action.error} retry={overview.reload} />
    {!data && overview.loading ? <Loading variant="panel" /> : !data ? <Empty title="云服务状态暂不可用" description="请刷新后再管理账号和资源。" /> : <>
      {!data.accounts.length && <Panel><Empty icon={CloudIcon} title="尚未添加云账号" description="添加账号后登记需要管理的资源。凭据只保存在面板，自动控制默认关闭。" /></Panel>}
      {data.accounts.map(account => <AccountPanel key={account.id} account={account} resources={data.resources.filter(resource => resource.account_id === account.id)} data={data} actions={actions} />)}
      {orphans.length > 0 && <AccountPanel resources={orphans} data={data} actions={actions} />}
      <Activity data={data} available={available} current={current} reload={overview.reload} />
    </>}
    <DialogPresence>{editing?.type === 'account' && <AccountEditor account={editing.value} available={available} current={current} refresh={overview.reload} close={() => setEditing(null)} saved={done} />}</DialogPresence>
    <DialogPresence>{editing?.type === 'resource' && data && <ResourceEditor resource={editing.value} accountId={editing.accountId} accounts={data.accounts} available={available} current={current} refresh={overview.reload} close={() => setEditing(null)} saved={done} />}</DialogPresence>
    <DialogPresence>{editing?.type === 'bandwidth' && <BandwidthEditor resource={editing.value} available={available} current={current} refresh={overview.reload} close={() => setEditing(null)} saved={done} />}</DialogPresence>
    <DialogPresence>{editing?.type === 'power' && <PowerEditor resource={editing.value} action={editing.action} available={available} current={current} refresh={overview.reload} close={() => setEditing(null)} saved={done} />}</DialogPresence>
    <DialogPresence>{editing?.type === 'eip-rotation' && <EipRotationEditor resource={editing.value} current={current} close={() => setEditing(null)} saved={done} />}</DialogPresence>
    <DialogPresence>{editing?.type === 'power-policy' && <PowerPolicyEditor resource={editing.value} available={available} current={current} refresh={overview.reload} close={() => setEditing(null)} saved={done} />}</DialogPresence>
    <DialogPresence>{resume && <Confirm title="恢复自动开机" confirmLabel="恢复自动开机" busyLabel="正在处理…" busy={action.busy} error={action.error} confirmDisabled={Boolean(resumeError())} onClose={() => setResume(null)} onConfirm={() => perform(resumeError, () => api(`/api/plugins/alicloud/resources/${resume.value.id}/power-resume`, 'POST', { revision: resume.value.revision }), done)}>解除“{resume.value.name}”的手动暂停后，已启用的每日计划或保活可能启动实例并产生费用。流量停机保护仍然有效。</Confirm>}</DialogPresence>
    <DialogPresence>{removing && <Confirm title={removing.type === 'accounts' ? '移除云账号' : '移除资源登记'} busy={action.busy} error={action.error} confirmDisabled={Boolean(removalError())} onClose={() => setRemoving(null)} onConfirm={() => perform(removalError, () => api(`/api/plugins/alicloud/${removing.type}/${removing.value.id}`, 'DELETE', { revision: removing.value.revision }), done)}>确认移除“{removing.value.name}”？这不会删除阿里云资源。账号须先移除全部资源登记；操作历史保留。</Confirm>}</DialogPresence>
  </div>
}
