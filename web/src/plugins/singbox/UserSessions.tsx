import { useEffect, useRef, useState } from 'react'
import { api } from '../../api'
import { ErrorNotice } from '../../components'
import { bytes } from '../../format'
import { resourceWriteError, useAction, useResource } from '../../hooks'
import { Button } from '@/components/ui/button'
import { Panel, PanelHeader, PanelTitle } from '@/components/panel'

type Connection = { id: string; principal: string; source: string; network: string; upload: number; download: number }
type Report = { observed_at: number; revision: number; truncated: boolean; closed: number; connections: Connection[] }
type Host = { server_id: number; name: string; online: boolean; supported: boolean; revision: number | null; operation: { expires_at: number; result: { error: string | null; sessions: Report | null } | null } | null }
export default function UserSessions({ userId, active }: { userId: number; active: boolean }) {
  const path = `/api/plugins/sing-box/users/${userId}/sessions`
  const resource = useResource<{ servers: Host[] }>(active ? path : null, 3000, { keep: true })
  const action = useAction()
  const previous = useRef<Record<number, Report>>({})
  const latest = useRef<Record<number, Report>>({})
  const [rates, setRates] = useState<Record<string, [number, number]>>({})
  useEffect(() => {
    const next: Record<string, [number, number]> = {}
    for (const host of resource.data?.servers ?? []) {
      const report = host.operation?.result?.sessions
      if (!report) continue
      if (latest.current[host.server_id]?.observed_at !== report.observed_at) { previous.current[host.server_id] = latest.current[host.server_id]; latest.current[host.server_id] = report }
      const prior = previous.current[host.server_id]
      const seconds = prior && report.revision === prior.revision ? report.observed_at - prior.observed_at : 0
      if (seconds <= 0) continue
      for (const item of report.connections) {
        const before = prior.connections.find(entry => entry.id === item.id)
        if (before) next[`${host.server_id}:${item.id}`] = [Math.max(0, item.upload - before.upload) / seconds, Math.max(0, item.download - before.download) / seconds]
      }
    }
    setRates(next)
  }, [resource.data])
  const request = (host: Host, close: boolean, ids: string[] = []) => {
    if (resourceWriteError(resource) || !host.revision) return
    void action.run(() => api(`${path}/${host.server_id}`, 'POST', { expected_revision: host.revision, close, ids }), resource.reload)
  }
  return <Panel className="mb-4"><PanelHeader><PanelTitle>代理连接</PanelTitle></PanelHeader><div className="space-y-4 p-4">
    <ErrorNotice message={resource.error || action.error} retry={resource.reload} />
    <p className="text-muted-foreground text-sm">查询服务器当前连接；再次查询后可计算两次采样间的平均速率。断开允许客户端重新连接，持续停用请使用暂停。每台服务器最多显示和处理 256 条连接。</p>
    {resource.data?.servers.map(host => {
      const operation = host.operation; const report = operation?.result?.sessions
      const expired = !operation?.result && operation && operation.expires_at * 1000 <= Date.now()
      const fresh = host.online && report && report.revision === host.revision && Date.now() - report.observed_at * 1000 <= 30000
      const busy = Boolean(operation && !operation.result && !expired)
      const disabled = action.busy || busy || !host.online || !host.supported || !host.revision || Boolean(resourceWriteError(resource))
      return <section key={host.server_id} className="min-w-0 rounded border p-3">
        <div className="flex flex-wrap items-center gap-2"><strong className="mr-auto break-all">{host.name}</strong><Button variant="outline" disabled={disabled} onClick={() => request(host, false)}>查询连接</Button><Button variant="outline" disabled={disabled || !fresh || !report?.connections.length} onClick={() => request(host, true)}>批量断开</Button></div>
        <p className="my-2 text-sm">{!host.online ? '服务器离线' : !host.supported ? '请升级 Agent' : busy ? '等待服务器回报…' : expired ? '查询已过期，请重新查询' : operation?.result?.error ? '操作失败，请检查运行时版本和部署状态' : report ? `${fresh ? '当前采样' : '历史采样'} · ${new Date(report.observed_at * 1000).toLocaleTimeString()} · ${report.connections.length} 条连接${report.truncated ? '（已截断，请分批处理）' : ''} · 本次确认断开 ${report.closed} 条` : '尚未查询'}</p>
        {report?.connections.map(item => { const rate = rates[`${host.server_id}:${item.id}`]; return <div key={item.id} className="flex flex-wrap items-center gap-2 border-t py-2 text-xs"><span className="min-w-0 flex-1 break-all">{item.network.toUpperCase()} · {item.source}<br />累计 ↑ {bytes(item.upload)} · ↓ {bytes(item.download)}{rate && fresh ? ` · 速率 ↑ ${bytes(rate[0])}/秒 · ↓ ${bytes(rate[1])}/秒` : ''}</span><Button size="sm" variant="outline" disabled={disabled || !fresh} onClick={() => request(host, true, [item.id])}>断开</Button></div> })}
      </section>
    })}
    {resource.data?.servers.length === 0 && <p>此用户尚无受管节点。</p>}
  </div></Panel>
}
