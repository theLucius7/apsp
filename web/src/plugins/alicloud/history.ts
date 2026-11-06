import type { CloudEvent, EipRotation, Operation, Overview, PowerJob } from './types'

export type ActivityItem =
  | { kind: 'bandwidth'; key: string; at: number; operation: Operation }
  | { kind: 'power'; key: string; at: number; job: PowerJob; event?: CloudEvent }
  | { kind: 'rotation'; key: string; at: number; rotation: EipRotation; event?: CloudEvent }
  | { kind: 'event'; key: string; at: number; event: CloudEvent }

const pendingStates = ['preview', 'queued', 'running', 'uncertain', 'unknown']
export const POWER_RESULT = 'ECS 启停结果'
/** Titles of the one notice a public IP rotation sends when it ends. */
export const ROTATION_RESULTS = ['公网 IP 已更换', '公网 IP 未更换，已恢复原地址', '公网 IP 更换未开始', '公网 IP 更换需要人工核对']

type Owned = { id: string; resource_id: string; created_at: number }
/** The latest work on the notice's resource created before it; such work never runs side by side on one resource. */
const owner = <T extends Owned>(work: T[], event: CloudEvent) =>
  work.filter(item => item.resource_id === event.resource_id && item.created_at <= event.created_at).sort((a, b) => b.created_at - a.created_at)[0]

/**
 * Bandwidth changes, power jobs, public IP rotations and cloud notices as one
 * history. A power result or rotation result notice only repeats its work's
 * state, so it is folded into that work: the latest of that kind on the same
 * resource created before the notice. Notices without such work (a CDT
 * threshold reminder, or work past the newest hundred) stay entries of their
 * own. Unfinished entries come first, then the newest.
 */
export function activityItems(data: Pick<Overview, 'operations' | 'power_jobs' | 'eip_rotations' | 'events'>): ActivityItem[] {
  const jobs = data.power_jobs ?? [], rotations = data.eip_rotations ?? []
  const items: ActivityItem[] = [
    ...data.operations.map(operation => ({ kind: 'bandwidth' as const, key: `operation:${operation.id}`, at: operation.created_at, operation })),
    ...jobs.map(job => ({ kind: 'power' as const, key: `power:${job.id}`, at: job.created_at, job })),
    ...rotations.map(rotation => ({ kind: 'rotation' as const, key: `rotation:${rotation.id}`, at: rotation.created_at, rotation })),
  ]
  const latest = new Map<string, CloudEvent>()
  for (const event of data.events ?? []) {
    const work = event.title === POWER_RESULT ? owner(jobs, event) : ROTATION_RESULTS.includes(event.title) ? owner(rotations, event) : undefined
    if (!work) { items.push({ kind: 'event', key: `event:${event.id}`, at: event.created_at, event }); continue }
    const key = `${event.title === POWER_RESULT ? 'power' : 'rotation'}:${work.id}`, previous = latest.get(key)
    if (!previous || previous.created_at < event.created_at || (previous.created_at === event.created_at && previous.id < event.id)) latest.set(key, event)
  }
  for (const item of items) if (item.kind === 'power' || item.kind === 'rotation') item.event = latest.get(item.key)
  const pending = (item: ActivityItem) => item.kind === 'bandwidth' ? pendingStates.includes(item.operation.status)
    : item.kind === 'power' ? pendingStates.includes(item.job.status) : item.kind === 'rotation' && pendingStates.includes(item.rotation.status)
  return items.sort((a, b) => Number(pending(b)) - Number(pending(a)) || b.at - a.at || a.key.localeCompare(b.key))
}

const deliveryStates: Record<string, string> = { pending: '等待投递', sent: '已送达', failed: '投递失败', cancelled: '已取消' }
/** External delivery of a notice; nothing when no channel is configured. */
export function deliveryText(event?: CloudEvent) {
  if (!event?.deliveries.length) return ''
  return `通知：${event.deliveries.map(d => `${d.channel === 'telegram' ? 'Telegram' : 'Webhook'} ${deliveryStates[d.status] ?? '状态未知'}${d.status !== 'sent' && d.attempts > 1 ? `（尝试 ${d.attempts} 次）` : ''}${d.last_error ? ` · ${d.last_error}` : ''}`).join('；')}`
}
