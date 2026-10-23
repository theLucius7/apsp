import { useRef, useState } from 'react'
import { api } from '../api'
import { Refresh, ErrorNotice, Loading } from '../components'
import { time } from '../format'
import { resourceWriteError, useAction, useResource } from '../hooks'
import { currentServerProbe, deleteServerProbe, probeReadError, probeLeaseNotice, saveServerProbe, bindProbeAuthorization, changeProbe, monitoringOf, withMonitoring, familyLabel, latency, loss, lossLabel, networkLabel, probeState, probeValue } from '../probes'
import type { Probe, ProbeResult, ProbeWriteSnapshot } from '../probes'
import ProbeMonitoringFields from './ProbeMonitoringFields'

const emptyProbe: Probe = { id: '00000000-0000-0000-0000-000000000000', name: '', kind: 'tcp', target: '', port: 443, interval_secs: 30, carrier: '', enabled: true, monitor: null }

export default function ServerProbes({ serverId }: { serverId: number }) {
  const base = `/api/servers/${serverId}`
  const probes = useResource<Probe[]>(`${base}/probes`)
  const results = useResource<ProbeResult[]>(`${base}/probe-results`)
  const probeAction = useAction()
  const [probe, setProbe] = useState<Probe>(emptyProbe)
  const writeSnapshot = useRef<ProbeWriteSnapshot>({ serverId, probes }); writeSnapshot.current = { serverId, probes }
  const [origin, setOrigin] = useState<Probe | null>(null)
  const [editing, setEditing] = useState(false)
  const [selected, setSelected] = useState<string | null>(null)
  const selectedHistory = useResource<ProbeResult[]>(selected ? `${base}/probe-results?probe_id=${encodeURIComponent(selected)}` : null, 15_000)
  const selectedResults = selectedHistory.data?.slice(0, 60)
  const writeError = (item?: Probe) => {
    const error = resourceWriteError(probes) || probeReadError(probes)
    if (error) return error
    if (item) { try { currentServerProbe(writeSnapshot.current, serverId, item) } catch (error) { return (error as Error).message } }
    return ''
  }
  return <>
    <section className="panel"><div className="panel-heading"><h2>持续网络拨测</h2><Refresh onClick={() => { probes.reload(); results.reload(); selectedHistory.reload() }} /><a className="button button-secondary button-small" href="#/latency">配置延迟检测</a></div><div className="panel-body">
      <ErrorNotice message={probes.error || results.error || writeError(editing ? probe : undefined) || probeAction.error} />
      <form onSubmit={event => { event.preventDefault(); if (writeError(editing ? probe : undefined)) return; void probeAction.run(() => saveServerProbe(writeSnapshot.current, serverId, editing ? origin : null, editing ? bindProbeAuthorization(probe) : probe, body => api<Probe>(editing ? `${base}/probes/${probe.id}` : `${base}/probes`, editing ? 'PATCH' : 'POST', body)), () => { setProbe(emptyProbe); setOrigin(null); setEditing(false); probes.reload() }) }}>
        <div className="form-grid"><label>名称<input required maxLength={128} value={probe.name} onChange={event => setProbe({ ...probe, name: event.target.value })} /></label>
          <label>方式<select disabled={editing} value={probe.kind} onChange={event => setProbe(changeProbe(probe, { kind: event.target.value as Probe['kind'], port: event.target.value === 'tcp' ? 443 : null }))}><option value="tcp">TCP 连接</option><option value="icmp">ICMP 回显</option></select></label>
          <label>目标地址<input required disabled={editing} maxLength={253} placeholder="主机名或 IP 地址" value={probe.target} onChange={event => setProbe(changeProbe(probe, { target: event.target.value }))} /></label>
          {probe.kind === 'tcp' && <label>端口<input required disabled={editing} type="number" min={1} max={65535} value={probe.port ?? 443} onChange={event => setProbe(changeProbe(probe, { port: Number(event.target.value) }))} /></label>}
          <label>间隔（秒）<input required type="number" min={10} max={3600} value={probe.interval_secs} onChange={event => setProbe({ ...probe, interval_secs: Number(event.target.value) })} /></label>
          <label>线路备注<input disabled={editing} maxLength={64} placeholder="如电信、联通、移动" value={probe.carrier} onChange={event => setProbe({ ...probe, carrier: event.target.value })} /></label></div>
        <ProbeMonitoringFields value={monitoringOf(probe)} onChange={monitoring => setProbe(withMonitoring(probe, monitoring))} editing={editing} />
        {editing && <p className="helper">方式、目标地址和端口创建后不可修改。更换目标请新建拨测，以保留历史归属。</p>}
        <button type="submit" className="button button-primary" disabled={probeAction.busy || Boolean(writeError(editing ? probe : undefined))}>{editing ? '保存拨测' : '添加拨测'}</button>{editing && <button type="button" className="button button-secondary" onClick={() => { setEditing(false); setOrigin(null); setProbe(emptyProbe) }}>取消编辑</button>}
      </form><p className="helper">{probeLeaseNotice}</p></div>
      {probes.loading && !probes.data ? <Loading /> : !probes.data?.length ? <div className="inline-empty">尚未配置拨测。</div> : <div className="table-wrap"><table><thead><tr><th>名称 / 线路</th><th>目标</th><th>延迟</th><th>丢包 / 连接失败率</th><th>最近测量</th><th>操作</th></tr></thead><tbody>{probes.data.map(item => {
        const latest = results.data?.filter(result => result.probe_id === item.id && result.sampled_at <= Date.now()).reduce<ProbeResult | undefined>((last, point) => !last || point.sampled_at > last.sampled_at ? point : last, undefined)
        const current = probeState(item, latest, Date.now(), Boolean(results.error)) === '最近采样' ? latest : undefined
        return <tr key={item.id}><td><button className="text-button" onClick={() => setSelected(selected === item.id ? null : item.id)}>{item.name}</button><small>{networkLabel(item)} · {item.monitor?.region || '地区未配置'} · {item.carrier || '未备注'}</small></td><td>{item.kind.toUpperCase()} {item.target}{item.port && `:${item.port}`}<small>{familyLabel(item, latest)}</small></td><td>{latency(probeValue(current, 'latency_ms'))}{latest?.error && <small>{latest.error}</small>}</td><td>{loss(probeValue(current, 'loss_percent'))}<small>{lossLabel(item)} · {probeState(item, latest, Date.now(), Boolean(results.error))}</small></td><td>{latest ? time(latest.sampled_at / 1000) : '等待设备测量'}</td><td>{item.task_id ? <a href="#/latency">统一任务管理</a> : <><button className="text-button" disabled={Boolean(writeError(item))} onClick={() => { if (writeError(item)) return; setProbe(item); setOrigin(item); setEditing(true) }}>编辑</button> <button className="text-button" disabled={probeAction.busy || Boolean(writeError(item))} onClick={() => { if (writeError(item)) return; void probeAction.run(() => saveServerProbe(writeSnapshot.current, serverId, item, { ...item, enabled: !item.enabled }, body => api(`${base}/probes/${item.id}`, 'PATCH', body)), probes.reload) }}>{item.enabled ? '暂停' : '启用'}</button> <button className="text-button" disabled={probeAction.busy || Boolean(writeError(item))} onClick={() => { if (writeError(item)) return; void probeAction.run(() => deleteServerProbe(writeSnapshot.current, serverId, item, (id, body) => api(`${base}/probes/${id}`, 'DELETE', body)), () => { probes.reload(); if (probe.id === item.id) { setEditing(false); setOrigin(null); setProbe(emptyProbe) } }) }}>删除</button></>}</td></tr>
      })}</tbody></table></div>}
      {selected && <div className="panel-body"><h3>最近 60 次测量</h3><ErrorNotice message={selectedHistory.error} />{selectedResults?.length ? <div className="table-wrap"><table><thead><tr><th>时间 / 网络版本</th><th>延迟</th><th>丢包 / 连接失败率</th></tr></thead><tbody>{selectedResults.map(item => <tr key={item.id}><td>{time(item.sampled_at / 1000)}<small>{item.address_family === 'ipv4' ? 'IPv4' : item.address_family === 'ipv6' ? 'IPv6' : '实际版本未知'}</small></td><td>{latency(probeValue(item, 'latency_ms'))}</td><td>{loss(probeValue(item, 'loss_percent'))}{item.error != null && <small>{item.error}</small>}</td></tr>)}</tbody></table></div> : <p>暂无测量记录。</p>}</div>}
    </section>
  </>
}
