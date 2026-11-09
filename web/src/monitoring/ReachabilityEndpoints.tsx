import { Badge } from '../components'
import { ago, time } from '../format'
import { deliveryLabels, detailLines, pendingNote, riskLabel, stateLabels, stateTones } from './reachability'
import type { ReachabilityEvent, ReachabilityTarget } from './reachability'

/** Each endpoint of a server as mainland China sees it, problems first (`sortTargets`). */
export function ReachabilityEndpoints({ targets }: { targets: ReachabilityTarget[] }) {
  return <ul className="grid gap-2" aria-label="端点可达性">{targets.map(target => {
    const pending = pendingNote(target), lines = detailLines(target.detail)
    return <li key={target.key} data-slot="reachability-endpoint" className="bg-card grid min-w-0 gap-x-4 gap-y-1.5 rounded-lg border px-3 py-2.5 text-xs sm:grid-cols-[minmax(0,1.2fr)_minmax(0,1fr)_auto]">
      <div className="min-w-0">
        <span className="text-foreground block text-sm wrap-anywhere">{target.nodes.map(node => node.name).join('、')}</span>
        <span className="text-muted-foreground block font-mono break-all">{target.host}:{target.port}{target.resolved_ip && target.resolved_ip !== target.host ? ` → ${target.resolved_ip}` : ''}</span>
      </div>
      <div className="text-muted-foreground min-w-0">{lines.length ? lines.map(line => <span key={line} className="block">{line}</span>) : '尚无结果'}</div>
      <div className="flex min-w-0 flex-wrap items-start gap-1.5 sm:flex-col sm:items-end">
        <Badge tone={stateTones[target.state]}>{stateLabels[target.state]}</Badge>
        <span className="text-muted-foreground" title={target.checked_at ? time(target.checked_at) : undefined}>{target.checked_at ? ago(target.checked_at) : '尚未检测'}{target.deferred_at ? ' · 因额度顺延' : ''}</span>
      </div>
      {pending && <p className="text-muted-foreground sm:col-span-3">{pending}</p>}
      {target.risks.length > 0 && <div className="flex flex-wrap gap-1.5 sm:col-span-3">{target.risks.map(risk => <Badge key={`${risk.code}:${risk.value}`} tone="warm" className="h-auto max-w-full break-all whitespace-normal">{riskLabel(risk)}</Badge>)}</div>}
    </li>
  })}</ul>
}

/** Block and recovery events with how each notification went; kept for 90 days. */
export function ReachabilityEvents({ events }: { events: ReachabilityEvent[] }) {
  return <ol className="divide-border divide-y" aria-label="可达性变化">{events.map(event => <li key={event.id} className="grid min-w-0 gap-x-4 gap-y-1 px-4 py-2.5 text-xs sm:grid-cols-[10rem_minmax(0,1fr)_auto]">
    <span className="text-muted-foreground tabular-nums">{time(event.created_at)}</span>
    <span className="min-w-0"><Badge tone={event.kind === 'blocked' ? 'bad' : 'good'}>{event.kind === 'blocked' ? '进入封锁' : '解除封锁'}</Badge><span className="mt-1 block whitespace-normal wrap-anywhere">{event.message}</span></span>
    <span className="text-muted-foreground">{event.deliveries.length ? event.deliveries.map(delivery => <span key={delivery.channel} className="block">{delivery.channel === 'telegram' ? 'Telegram' : 'Webhook'}：{deliveryLabels[delivery.state] ?? delivery.state}</span>) : '未配置通知渠道'}</span>
  </li>)}</ol>
}
