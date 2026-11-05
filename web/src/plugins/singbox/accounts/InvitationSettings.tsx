import { Panel, PanelTitle } from '@/components/panel'
import { Button } from '@/components/ui/button'
import { Checkbox } from '@/components/ui/checkbox'
import { Input } from '@/components/ui/input'
import { NativeSelect } from '@/components/ui/native-select'
import { useState } from 'react'
import { api } from '../../../api'
import { ErrorNotice, Field } from '../../../components'
import { settingsPoll, useAction, useResource } from '../../../hooks'
import type { PackageGroup, PolicyGroup } from '../groupTypes'
import { root } from './types'

type Settings = { enabled: boolean; package_group_id: number | null; policy_group_ids: number[]; default_quota: number; version: number }

/** What an account created from an invitation code starts with. */
export default function InvitationSettings() {
  const settings = useResource<Settings>(`${root}/invitation-settings`, settingsPoll)
  const policies = useResource<PolicyGroup[]>(`${root}/policy-groups`, settingsPoll)
  const packages = useResource<PackageGroup[]>(`${root}/package-groups`, settingsPoll)
  const action = useAction()
  const [groups, setGroups] = useState<number[] | null>(null)
  const value = settings.data
  const chosen = groups ?? value?.policy_group_ids ?? []
  return <Panel><div className="flex min-h-14 flex-wrap items-center justify-between gap-3 border-b px-4 py-3"><PanelTitle>邀请</PanelTitle></div><div className="space-y-3 p-4">
    <ErrorNotice message={settings.error || policies.error || packages.error || action.error} retry={settings.reload} />
    {value && <form key={value.version} onSubmit={event => {
      event.preventDefault(); if (!settings.isCurrent()) return
      const form = new FormData(event.currentTarget)
      void action.run(() => api(`${root}/invitation-settings`, 'PUT', { enabled: form.get('enabled') === 'on', package_group_id: form.get('package') ? Number(form.get('package')) : null, policy_group_ids: chosen, default_quota: Number(form.get('quota')), version: value.version }), () => { setGroups(null); settings.reload() })
    }}>
      <label className="my-3 flex items-center gap-2.5 text-sm"><Checkbox name="enabled" defaultChecked={value.enabled} />允许代理用户用邀请名额生成邀请码</label>
      <Field label="受邀账号的套餐" hint="原系统兼容期间只能选已对应原权限组的套餐。不选则由管理员事后分配。"><NativeSelect className="w-full" name="package" defaultValue={value.package_group_id ?? ''}><option value="">不自动分配</option>{packages.data?.map(entry => <option key={entry.id} value={entry.id}>{entry.name}</option>)}</NativeSelect></Field>
      {policies.data?.length ? <fieldset className="my-3 grid gap-1.5 border-0 p-0 sm:grid-cols-2"><legend className="mb-2 text-sm font-medium">受邀账号的策略组</legend>{policies.data.map(entry => <label key={entry.id} className="flex cursor-pointer items-center gap-2.5 text-sm"><Checkbox checked={chosen.includes(entry.id)} onCheckedChange={checked => setGroups(checked === true ? [...chosen, entry.id] : chosen.filter(id => id !== entry.id))} />{entry.name}</label>)}</fieldset> : null}
      <Field label="受邀账号自带的邀请名额" hint="0 表示受邀的人不能再邀请别人。"><Input name="quota" type="number" min={0} max={1000} required defaultValue={value.default_quota} /></Field>
      <p className="text-muted-foreground text-xs leading-relaxed">每个代理用户的邀请名额在「代理用户 › 邀请」中调整。原来的钱包余额已按当时每份邀请的价格换算成名额。</p>
      <Button disabled={action.busy || !settings.isCurrent()}>保存邀请设置</Button>
    </form>}
  </div></Panel>
}
