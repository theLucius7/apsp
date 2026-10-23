import type { Probe, ProbeAuthorization, ProbeMonitor } from '../probes'
import { authorizationMatches, changeProbe, probeIdentity } from '../probes'
import { Checkbox } from '@/components/ui/checkbox'
import { Input } from '@/components/ui/input'
import { NativeSelect } from '@/components/ui/native-select'

export default function ProbeAuthorizationFields({ probe, onChange, editing = false }: { probe: Probe; onChange: (probe: Probe) => void; editing?: boolean }) {
  const monitor: ProbeMonitor = probe.monitor ?? { region: '', address_family: 'any', authorization: null }
  const authorization: ProbeAuthorization = monitor.authorization ?? { kind: 'owned', source: '', scope: '', enabled: false, expires_at: null,
    identity: { kind: probe.kind, target: probe.target, port: probe.port, address_family: monitor.address_family } }
  const update = (part: Partial<ProbeMonitor>) => onChange(changeProbe(probe, { monitor: { ...monitor, ...part } }))
  const authorize = (part: Partial<ProbeAuthorization>) => onChange({ ...probe, ...(part.enabled === false ? { enabled: false } : {}), monitor: { ...monitor, authorization: { ...authorization, ...part, ...(part.enabled === true ? { identity: probeIdentity(probe) } : {}) } } })
  const expiry = authorization.expires_at == null ? null : new Date(authorization.expires_at * 1000)
  return <>
    <div className="grid gap-x-4 sm:grid-cols-2">
      <label className="grid min-w-0 gap-1.5 text-sm">地区<Input maxLength={64} disabled={editing} value={monitor.region} onChange={event => update({ region: event.target.value })} placeholder="留空表示未知" /></label>
      <label className="grid min-w-0 gap-1.5 text-sm">地址家族<NativeSelect className="w-full" disabled={editing} value={monitor.address_family} onChange={event => update({ address_family: event.target.value as ProbeMonitor['address_family'] })}><option value="any">自动选择</option><option value="ipv4">IPv4</option><option value="ipv6">IPv6</option></NativeSelect></label>
      <label className="grid min-w-0 gap-1.5 text-sm">目标授权类型<NativeSelect className="w-full" value={authorization.kind} onChange={event => authorize({ kind: event.target.value as ProbeAuthorization['kind'] })}><option value="owned">自有目标</option><option value="consent">已取得目标所有者同意</option></NativeSelect></label>
      <label className="grid min-w-0 gap-1.5 text-sm">授权来源<Input required={probe.enabled} maxLength={256} value={authorization.source} onChange={event => authorize({ source: event.target.value })} placeholder="资产登记或授权记录编号，不含密钥" /></label>
      <label className="grid min-w-0 gap-1.5 text-sm">授权范围<Input required={probe.enabled} maxLength={512} value={authorization.scope} onChange={event => authorize({ scope: event.target.value })} placeholder="允许的方式、频率和用途" /></label>
      <label className="grid min-w-0 gap-1.5 text-sm">授权到期（UTC）<Input type="datetime-local" min="1970-01-01T00:00" max="9999-12-31T23:59" value={expiry && Number.isFinite(expiry.getTime()) ? expiry.toISOString().slice(0, 16) : ''} onChange={event => authorize({ expires_at: event.target.value ? Math.floor(Date.parse(event.target.value + 'Z') / 1000) : null })} /><small>留空表示授权未设到期，请按实际记录填写。</small></label>
    </div>
    <label className="flex items-center gap-2 text-sm"><Checkbox required={probe.enabled} checked={authorization.enabled && authorizationMatches(probe)} onCheckedChange={checked => authorize({ enabled: checked === true })} />确认有权按上述范围检测此目标</label>
    <p className="text-muted-foreground mt-3 text-xs leading-relaxed">不预置公共测速服务器。改变目标、端口、方式或地址家族后须重新勾选确认。执行许可最长 90 秒；断连、授权撤销或许可到期会取消在途检测，冷启动须重新取得许可，历史保留。填写记录不等于系统代为取得授权。旧 Agent 需先升级到支持短期拨测许可的版本。</p>
  </>
}
