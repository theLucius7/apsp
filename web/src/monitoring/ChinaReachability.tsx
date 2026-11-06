import { useState } from 'react'
import { api } from '../api'
import { Badge, Empty, ErrorNotice, Field, Loading, PageHeader, Refresh, SettingRow } from '../components'
import { ago, time } from '../format'
import { resourceWriteError, useAction, useResource } from '../hooks'
import LatencyTabs from './LatencyTabs'
import { deliveryLabels, detailLines, intervalError, pendingNote, riskLabel, runErrorText, runLine, sortTargets, stateLabels, stateTones } from './reachability'
import type { ReachabilityOverview, ReachabilitySettings, ReachabilityTarget } from './reachability'
import { Panel, PanelHeader, PanelTitle } from '@/components/panel'
import { Button } from '@/components/ui/button'
import { Checkbox } from '@/components/ui/checkbox'
import { Input } from '@/components/ui/input'
import { Switch } from '@/components/ui/switch'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'
import { GlobeIcon, PlayIcon } from 'lucide-react'

const root = '/api/plugins/sing-box/reachability'

function SettingsForm({ settings, writeError, onSaved }: { settings: ReachabilitySettings; writeError: () => string; onSaved: () => void }) {
  const [enabled, setEnabled] = useState(settings.enabled)
  const [interval, setIntervalMinutes] = useState(String(settings.interval_minutes))
  const [token, setToken] = useState('')
  const [clearToken, setClearToken] = useState(false)
  const action = useAction()
  const invalid = intervalError(interval, settings)
  const blocked = writeError()
  return <Panel aria-label="检测设置"><PanelHeader><PanelTitle>检测设置</PanelTitle></PanelHeader>
    <form className="p-4" onSubmit={event => {
      event.preventDefault()
      if (invalid || writeError()) return
      void action.run(() => api(`${root}/settings`, 'PUT', { enabled, interval_minutes: Number(interval), revision: settings.revision, ...(token ? { api_token: token } : {}), ...(clearToken ? { clear_token: true } : {}) }), onSaved)
    }}>
      <ErrorNotice message={action.error || blocked} />
      <fieldset disabled={action.busy}>
        <SettingRow title="启用中国可达性检测" description="默认关闭。开启后，面板会把节点的 IP 地址和端口发送给第三方服务 Globalping（api.globalping.io），由其在中国大陆和海外的探测点发起 TCP 连接。域名先在面板解析，只发送解析结果；不发送节点名称、密钥或用户信息。"><Switch checked={enabled} onCheckedChange={setEnabled} /></SettingRow>
        <div className="mt-4 grid gap-4 sm:grid-cols-2">
          <Field className="m-0" label="检测间隔（分钟）" hint={`最少 ${settings.minimum_interval_minutes} 分钟。额度不足时剩余节点顺延，不算失败。`}><Input required type="number" min={settings.minimum_interval_minutes} max={settings.maximum_interval_minutes} step={1} value={interval} onChange={event => setIntervalMinutes(event.target.value)} /></Field>
          <Field className="m-0" label="Globalping API 令牌（可选）" hint={settings.token_configured ? '已保存，只写不回显；留空保留。填写新令牌后旧令牌立即停用。' : '匿名额度为每小时 250 次测试，每个探测点算一次；令牌可提高额度。加密保存在凭据中心。'}><Input type="password" autoComplete="off" spellCheck={false} maxLength={512} disabled={clearToken} value={token} onChange={event => setToken(event.target.value.trim())} /></Field>
        </div>
        {settings.token_configured && <label className="mt-3 flex items-center gap-2 text-sm"><Checkbox checked={clearToken} onCheckedChange={checked => { setClearToken(checked === true); if (checked === true) setToken('') }} />清除已保存的令牌</label>}
        <p className="text-destructive mt-3 text-xs leading-relaxed">{invalid}</p>
      </fieldset>
      <div className="mt-3 flex justify-end"><Button disabled={action.busy || Boolean(invalid) || Boolean(blocked)}>{action.busy ? '正在保存…' : '保存设置'}</Button></div>
    </form>
  </Panel>
}

function TargetRow({ target }: { target: ReachabilityTarget }) {
  const pending = pendingNote(target)
  const lines = detailLines(target.detail)
  return <TableRow>
    <TableCell className="min-w-56 whitespace-normal"><span className="block">{target.nodes.map(node => node.name).join('、')}</span><small className="text-muted-foreground block font-mono text-xs break-all">{target.host}:{target.port}{target.resolved_ip && target.resolved_ip !== target.host ? ` → ${target.resolved_ip}` : ''}</small><small className="text-muted-foreground block text-xs">{[...new Set(target.nodes.map(node => node.server_name))].join('、')}</small></TableCell>
    <TableCell><Badge tone={stateTones[target.state]}>{stateLabels[target.state]}</Badge>{pending && <small className="text-muted-foreground mt-1 block text-xs">{pending}</small>}</TableCell>
    <TableCell>{lines.length ? lines.map(line => <span key={line} className="block text-xs">{line}</span>) : <span className="text-muted-foreground text-xs">尚无结果</span>}</TableCell>
    <TableCell>{target.checked_at ? <span title={time(target.checked_at)}>{ago(target.checked_at)}</span> : '尚未检测'}{target.deferred_at && <small className="text-muted-foreground block text-xs">因额度顺延到下一轮</small>}</TableCell>
    <TableCell className="min-w-40 whitespace-normal">{target.risks.length ? target.risks.map(risk => <Badge key={`${risk.code}:${risk.value}`} tone="warm" className="mb-1 block h-auto w-fit max-w-full break-all whitespace-normal">{riskLabel(risk)}</Badge>) : <span className="text-muted-foreground text-xs">无</span>}</TableCell>
  </TableRow>
}

export default function ChinaReachability() {
  const actor = useResource<{ role: string }>('/api/control-center/me', 0)
  const owner = actor.data?.role === 'owner'
  const overview = useResource<ReachabilityOverview>(owner ? root : null, 15_000)
  const action = useAction()
  const settings = overview.data?.settings
  const last = settings ? runLine(settings.last_run) : undefined
  const active = settings?.running || settings?.run_requested
  const writeError = () => resourceWriteError(overview)
  if (actor.data && !owner) return <><PageHeader title="延迟检测" /><Empty icon={GlobeIcon} title="需要所有者会话" description="这里会显示节点地址并能把地址发送给第三方，只对所有者开放。" /></>
  return <><PageHeader title="延迟检测">
    <Refresh onClick={overview.reload} />
    <Button disabled={!settings?.enabled || action.busy || Boolean(active) || Boolean(writeError())} title={settings?.enabled ? undefined : '启用检测后才能运行'} onClick={() => { if (!writeError()) void action.run(() => api(`${root}/run`, 'POST'), overview.reload) }}><PlayIcon aria-hidden="true" />{active ? '检测进行中…' : '立即检测'}</Button>
  </PageHeader>
  <LatencyTabs active="reachability" />
  <ErrorNotice message={actor.error || overview.error || action.error} retry={overview.reload} />
  {settings && <SettingsForm key={settings.revision} settings={settings} writeError={writeError} onSaved={overview.reload} />}
  <Panel aria-label="节点可达性"><PanelHeader><PanelTitle>节点可达性</PanelTitle><span className="text-muted-foreground text-xs">{overview.data ? `${overview.data.targets.length} 个端点` : '—'}{settings?.enabled ? '' : ' · 未启用，不会发送任何地址'}</span></PanelHeader>
    {settings?.last_run.error && <p className="text-destructive px-4 pt-3 text-xs">{runErrorText(settings.last_run.error)}</p>}
    {last && <p className="text-muted-foreground px-4 pt-3 text-xs">最近一轮{settings?.last_run.finished_at ? `（${ago(settings.last_run.finished_at)}）` : ''}：{last}</p>}
    {!overview.data && overview.loading ? <Loading variant="table" rows={3} /> : !overview.data?.targets.length ? <Empty icon={GlobeIcon} title="没有可检测的节点" description="启用的原生节点和已导入的 Nexus 节点会列在这里，同一地址和端口只检测一次。" /> : <div className="max-w-full min-w-0"><Table><TableHeader><TableRow><TableHead>节点</TableHead><TableHead>状态</TableHead><TableHead>运营商详情</TableHead><TableHead>最近检查</TableHead><TableHead>风险提示</TableHead></TableRow></TableHeader><TableBody>{sortTargets(overview.data.targets).map(target => <TargetRow key={target.key} target={target} />)}</TableBody></Table></div>}
    <p className="text-muted-foreground p-4 text-xs leading-relaxed">连续两轮失败才判定为封锁，一轮恢复即解除。关闭的端口和被过滤的端口对探测点看起来一样，所以只有海外确认开放的对照端口才用于判断 IP 封锁。风险提示对应 Xray-core 的 REALITY 配置警告，只提醒，不阻止任何操作。</p>
  </Panel>
  {Boolean(overview.data?.events.length) && <Panel aria-label="最近告警"><PanelHeader><PanelTitle>最近告警</PanelTitle><span className="text-muted-foreground text-xs">保留 90 天，通过已配置的通知渠道发送</span></PanelHeader><div className="max-w-full min-w-0"><Table><TableHeader><TableRow><TableHead>时间</TableHead><TableHead>事件</TableHead><TableHead>通知</TableHead></TableRow></TableHeader><TableBody>{overview.data?.events.map(event => <TableRow key={event.id}><TableCell>{time(event.created_at)}</TableCell><TableCell><Badge tone={event.kind === 'blocked' ? 'bad' : 'good'}>{event.kind === 'blocked' ? '进入封锁' : '解除封锁'}</Badge><small className="mt-1 block text-xs whitespace-normal">{event.message}</small></TableCell><TableCell>{event.deliveries.length ? event.deliveries.map(delivery => <span key={delivery.channel} className="block text-xs">{delivery.channel === 'telegram' ? 'Telegram' : 'Webhook'}：{deliveryLabels[delivery.state] ?? delivery.state}</span>) : <span className="text-muted-foreground text-xs">未配置渠道</span>}</TableCell></TableRow>)}</TableBody></Table></div></Panel>}
  </>
}
