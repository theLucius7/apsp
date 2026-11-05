import type { CloudEvent, Operation, Overview, PowerJob } from './types'

export type ActivityItem =
  | { kind: 'bandwidth'; key: string; at: number; operation: Operation }
  | { kind: 'power'; key: string; at: number; job: PowerJob; event?: CloudEvent }
  | { kind: 'event'; key: string; at: number; event: CloudEvent }

const pendingStates = ['preview', 'queued', 'running', 'uncertain']
export const POWER_RESULT = 'ECS 启停结果'

/**
 * Bandwidth changes, power jobs and cloud notices as one history. A power
 * result notice only repeats its job's state, so it is folded into that job:
 * jobs on one resource never run side by side, so its job is the latest one
 * on the same resource created before the notice. Notices without such a job
 * (a CDT threshold reminder, or a job past the newest hundred) stay entries of
 * their own. Unfinished entries come first, then the newest.
 */
export function activityItems(data: Pick<Overview, 'operations' | 'power_jobs' | 'events'>): ActivityItem[] {
  const jobs = data.power_jobs ?? []
  const items: ActivityItem[] = [
    ...data.operations.map(operation => ({ kind: 'bandwidth' as const, key: `operation:${operation.id}`, at: operation.created_at, operation })),
    ...jobs.map(job => ({ kind: 'power' as const, key: `power:${job.id}`, at: job.created_at, job })),
  ]
  const latest = new Map<string, CloudEvent>()
  for (const event of data.events ?? []) {
    const job = event.title !== POWER_RESULT ? undefined
      : jobs.filter(job => job.resource_id === event.resource_id && job.created_at <= event.created_at).sort((a, b) => b.created_at - a.created_at)[0]
    if (!job) { items.push({ kind: 'event', key: `event:${event.id}`, at: event.created_at, event }); continue }
    const previous = latest.get(job.id)
    if (!previous || previous.created_at < event.created_at || (previous.created_at === event.created_at && previous.id < event.id)) latest.set(job.id, event)
  }
  for (const item of items) if (item.kind === 'power') item.event = latest.get(item.job.id)
  const pending = (item: ActivityItem) => item.kind === 'bandwidth' ? pendingStates.includes(item.operation.status) : item.kind === 'power' && pendingStates.includes(item.job.status)
  return items.sort((a, b) => Number(pending(b)) - Number(pending(a)) || b.at - a.at || a.key.localeCompare(b.key))
}

const deliveryStates: Record<string, string> = { pending: '等待投递', sent: '已送达', failed: '投递失败', cancelled: '已取消' }
/** External delivery of a notice; nothing when no channel is configured. */
export function deliveryText(event?: CloudEvent) {
  if (!event?.deliveries.length) return ''
  return `通知：${event.deliveries.map(d => `${d.channel === 'telegram' ? 'Telegram' : 'Webhook'} ${deliveryStates[d.status] ?? '状态未知'}${d.status !== 'sent' && d.attempts > 1 ? `（尝试 ${d.attempts} 次）` : ''}${d.last_error ? ` · ${d.last_error}` : ''}`).join('；')}`
}
