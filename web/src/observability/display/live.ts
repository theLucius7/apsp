import type { Server } from '../../types'

export type LiveServer = Pick<Server, 'id' | 'online' | 'last_seen' | 'last_heartbeat_at' | 'metrics_stale' | 'metrics_sampled_at' | 'metrics_received_at' | 'metrics_persisted_at' | 'latest_metrics'>
export type LiveSnapshot = { served_at: number; public_view: boolean; servers: LiveServer[] }

function latestSample(previous: LiveServer | undefined, incoming: LiveServer, servedAt: number): LiveServer {
  if (!previous || previous.metrics_sampled_at === null || incoming.metrics_sampled_at !== null && incoming.metrics_sampled_at >= previous.metrics_sampled_at) return incoming
  const age = servedAt - previous.metrics_sampled_at
  // The incoming stale flag describes an older sample. A retained sample is
  // fresh only within the panel's minimum allowance; public rows omit intervals.
  // Age it on every response even when the older sample is reported as fresh.
  const stale = previous.metrics_stale || !Number.isFinite(age) || age < 0 || age > 15_000
  return { ...incoming, metrics_sampled_at: previous.metrics_sampled_at, latest_metrics: previous.latest_metrics,
    metrics_received_at: previous.metrics_received_at, metrics_stale: stale }
}

export function mergeLiveSnapshot(previous: LiveSnapshot | undefined, incoming: LiveSnapshot): LiveSnapshot {
  if (!previous || previous.public_view !== incoming.public_view) return incoming
  const samples = new Map(previous.servers.map(server => [server.id, server]))
  // Visibility and connection state always come from the new response. Only
  // newer real samples survive an out-of-order batch or a delayed persistence read.
  return { ...incoming, servers: incoming.servers.map(server => latestSample(samples.get(server.id), server, incoming.served_at)) }
}

export function mergeLiveServers(metadata: Server[], snapshot: LiveSnapshot): Server[] {
  const live = new Map(snapshot.servers.map(server => [server.id, server]))
  return metadata.filter(server => !server.asset_settings?.hidden && live.has(server.id) && Boolean(server.public_view) === snapshot.public_view).map(server => {
    const next = latestSample(server, live.get(server.id)!, snapshot.served_at)
    // Only the explicit live projection can update metadata. Never spread a
    // remote record into a public server or replay an older metric sample.
    return { ...server, served_at: snapshot.served_at, online: next.online, last_seen: next.last_seen, last_heartbeat_at: next.last_heartbeat_at, metrics_stale: next.metrics_stale,
      metrics_received_at: next.metrics_received_at, metrics_persisted_at: next.metrics_persisted_at,
      metrics_sampled_at: next.metrics_sampled_at, latest_metrics: next.latest_metrics }
  })
}
