export type TrafficSample = { bucket_start: number; sampled_at: number; mainland_bytes: string; overseas_bytes: string }
export type TrafficGroup = { seconds: number; from: number; to: number; samples: TrafficSample[] }
export type TrafficHistory = { account_id: string; queried_at: number; groups: TrafficGroup[] }
const month = (at: number) => new Date((at + 28800) * 1000).toISOString().slice(0, 7)
const count = (value: string | undefined) => value && /^\d+$/.test(value) ? BigInt(value) : null

export function trafficPoints(group: TrafficGroup, pool: 'mainland_bytes' | 'overseas_bytes', delta: boolean) {
  const rows = new Map(group.samples.map(sample => [sample.bucket_start, sample]))
  const points: { at: number; value: number | null }[] = []
  for (let at = group.from; at <= group.to; at += group.seconds) {
    const current = rows.get(at), previous = rows.get(at - group.seconds)
    let value = count(current?.[pool])
    const prior = count(previous?.[pool])
    const reset = current && previous && month(current.sampled_at) !== month(previous.sampled_at)
    if (delta) value = value !== null && prior !== null && !reset && value >= prior ? value - prior : null
    else if (reset) points.push({ at: at * 1000 - 1, value: null })
    points.push({ at: at * 1000, value: value === null ? null : Number(value) / 1024 ** 3 })
  }
  return points
}

export function readingStale(at: number | null | undefined, now: number, maxAge: number, error?: string | null, monthly = false) {
  return Boolean(error || !at || !Number.isSafeInteger(at) || at > now || now - at > maxAge || (monthly && month(at) !== month(now)))
}
