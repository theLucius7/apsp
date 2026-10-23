import { Field, SettingRow } from '../components'
import { emptyMonitoring, networks } from '../probes'
import type { ProbeMonitoring, ProbeNetwork } from '../probes'
import { Input } from '@/components/ui/input'
import { NativeSelect } from '@/components/ui/native-select'
import { Switch } from '@/components/ui/switch'

export default function ProbeMonitoringFields({ value, onChange, editing = false }: { value?: ProbeMonitoring; onChange: (value: ProbeMonitoring) => void; editing?: boolean }) {
  const current = value ?? emptyMonitoring(), authorization = current.authorization
  const change = (part: Partial<ProbeMonitoring>) => onChange({ ...current, ...part, ...(part.ip_version && part.ip_version !== current.ip_version ? { authorization: { ...authorization, confirmed: false } } : {}) })
  const authorize = (part: Partial<ProbeMonitoring['authorization']>) => change({ authorization: { ...authorization, confirmed: false, ...part } })
  const expiry = authorization.expires_at == null ? null : new Date(authorization.expires_at * 1000 - new Date().getTimezoneOffset() * 60_000)
  const date = expiry && Number.isFinite(expiry.getTime()) ? expiry.toISOString().slice(0, 16) : ''
  return <>
    <div className="grid gap-x-4 sm:grid-cols-2">
      <Field label="运营商线路"><NativeSelect className="w-full" disabled={editing} value={current.network} onChange={event => change({ network: event.target.value as ProbeNetwork })}>{networks.map(([key, label]) => <option key={key} value={key}>{label}</option>)}</NativeSelect></Field>
      <Field label="目标地区" hint="手动标注目标所在地区；不推断线路或排名。"><Input required={current.network !== 'other'} disabled={editing} maxLength={64} value={current.region} onChange={event => change({ region: event.target.value })} placeholder="例如：华东 · 自有目标" /></Field>
      <Field label="网络版本"><NativeSelect className="w-full" disabled={editing} value={current.ip_version} onChange={event => change({ ip_version: event.target.value as ProbeMonitoring['ip_version'] })}><option value="auto">自动选择 IPv4 / IPv6</option><option value="ipv4">仅 IPv4</option><option value="ipv6">仅 IPv6</option></NativeSelect></Field>
      <Field label="目标授权依据"><NativeSelect className="w-full" value={authorization.basis} onChange={event => authorize({ basis: event.target.value as ProbeMonitoring['authorization']['basis'] })}><option value="unconfirmed">尚未确认授权</option><option value="owned">自有目标</option><option value="permission">已获得明确许可</option></NativeSelect></Field>
      <Field label="授权来源" hint="填写所有权记录或许可记录的引用，不填密钥。"><Input maxLength={256} value={authorization.source} onChange={event => authorize({ source: event.target.value })} placeholder="所有权记录或许可文件编号" /></Field>
      <Field label="授权适用范围" hint="明确覆盖本目标、方式、频率及执行服务器。"><Input maxLength={512} value={authorization.scope} onChange={event => authorize({ scope: event.target.value })} placeholder="目标 / TCP 或 ICMP / 周期 / 允许执行的服务器" /></Field>
      <Field label="授权到期时间（可选）"><Input type="datetime-local" value={date} onChange={event => authorize({ expires_at: event.target.value ? Math.floor(new Date(event.target.value).getTime() / 1000) : null })} /></Field>
    </div>
    <SettingRow title="确认该范围内允许周期探测" description="公开可达不代表获得许可。未确认、撤销或到期时保留历史并停止调度。"><Switch required={!editing} checked={authorization.confirmed} disabled={authorization.basis === 'unconfirmed' || !authorization.source.trim() || !authorization.scope.trim()} onCheckedChange={checked => authorize({ confirmed: checked })} /></SettingRow>
    <p className="text-muted-foreground mt-3 text-xs leading-relaxed">目标默认留空。每轮最多 4 次、每节点最多 4 个并发，每次连接 1 秒、每轮总计最多 12 秒；仅执行 TCP / ICMP 轻量监控。需要支持短期拨测许可的新版 Agent；执行许可绑定设备、连接会话和配置修订，最长 90 秒。断连、许可到期或授权撤销后取消在途检测；冷启动须重新取得许可，历史保留。{editing && '运营商、地区与网络版本固定；更换后请新建目标以隔离历史。'}</p>
  </>
}
