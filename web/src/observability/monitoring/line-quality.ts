// Line quality from mainland China (ADR 0130): each server's latency and loss to the
// three carriers, measured by the server, beside the reachability of its endpoints,
// measured from China. Both answer one question, so one row holds both.
import { probeSlots, probeState, probeValue } from '../probes'
import type { Probe, ProbeOverview, ProbeResult } from '../probes'
import { stateLabels, worstState } from './reachability'
import type { ReachabilityState, ReachabilityTarget } from './reachability'

export type Carrier = 'telecom' | 'unicom' | 'mobile'
export const carriers: readonly (readonly [Carrier, string])[] = [['telecom', '电信'], ['unicom', '联通'], ['mobile', '移动']]

/** Recent average loss at or above this marks a line as a problem. */
export const lossThreshold = 5

const live = '最近采样'
// States that say nothing is wrong with the line itself: a paused task, a probe that has
// not sampled yet, and an overview that could not be read (the page reports that once).
const quiet = new Set(['已暂停', '等待采样', '状态未知'])
const troubled = new Set<ReachabilityState>(['ip_blocked', 'port_blocked', 'unconfirmed', 'partial', 'service_down'])

/** One probe of one server: the latest reading and the recent samples behind it. */
export type LineSample = {
  probe: Probe
  latest?: ProbeResult
  state: string
  latency: number | null
  loss: number | null
  /** Average loss over the recent samples that measured anything. */
  recentLoss: number | null
  slots: (ProbeResult | undefined)[]
}

export type LineServer = { id: number; name: string; online: boolean; region: string; group: string }

export type ServerLine = {
  server: LineServer
  carriers: Record<Carrier, LineSample[]>
  /** Probes to other networks, and probes without a carrier. */
  others: LineSample[]
  endpoints: ReachabilityTarget[]
  /** The worst state of the server's endpoints; null without endpoints. */
  reachability: ReachabilityState | null
  problems: string[]
}

type ServerInput = { id: number; name: string; online?: boolean; asset_settings?: { region?: string; group_name?: string } | null }

function latestOf(results: ProbeResult[], probe: Probe, now: number) {
  return results.filter(point => point.probe_id === probe.id && point.sampled_at <= now)
    .reduce<ProbeResult | undefined>((last, point) => !last || point.sampled_at > last.sampled_at ? point : last, undefined)
}

export function lineSample(probe: Probe, results: ProbeResult[], now: number, unavailable = false): LineSample {
  const latest = latestOf(results, probe, now)
  const state = probeState(probe, latest, now, unavailable)
  const current = state === live ? latest : undefined
  const slots = probeSlots(results, probe, now)
  const losses = slots.map(point => probeValue(point, 'loss_percent')).filter((value): value is number => value !== null)
  return {
    probe, latest, state, slots,
    latency: probeValue(current, 'latency_ms'),
    loss: probeValue(current, 'loss_percent'),
    recentLoss: losses.length ? losses.reduce((sum, value) => sum + value, 0) / losses.length : null,
  }
}

const carrierOf = (probe: Probe): Carrier | null => {
  const network = probe.monitor?.network
  return network === 'telecom' || network === 'unicom' || network === 'mobile' ? network : null
}

const byName = (a: Probe, b: Probe) => a.name.localeCompare(b.name, 'zh-CN')

function sampleProblem(label: string, sample: LineSample) {
  if (sample.state !== live) return quiet.has(sample.state) ? null : `${label}${sample.state}`
  return sample.recentLoss !== null && sample.recentLoss >= lossThreshold ? `${label}丢包 ${sample.recentLoss.toFixed(1)}%` : null
}

/** Why a row needs attention; empty when nothing does. Absolute latency is not a problem: it depends on where the server is. */
export function lineProblems(line: Omit<ServerLine, 'problems'>) {
  if (!line.server.online) return ['服务器离线']
  const problems: string[] = []
  for (const [carrier, label] of carriers) for (const sample of line.carriers[carrier]) {
    const problem = sampleProblem(label, sample)
    if (problem) problems.push(problem)
  }
  for (const sample of line.others) {
    const problem = sampleProblem(`${sample.probe.name}：`, sample)
    if (problem) problems.push(problem)
  }
  if (line.reachability && troubled.has(line.reachability)) problems.push(`中国可达性：${stateLabels[line.reachability]}`)
  return problems
}

/** One row per server, in the order of `servers`. */
export function serverLines(servers: ServerInput[], overview: ProbeOverview[] | undefined, targets: ReachabilityTarget[] | undefined, now: number, unavailable = false): ServerLine[] {
  return servers.map(server => {
    const rows = overview?.filter(row => row.server_id === server.id) ?? []
    const lines: Record<Carrier, LineSample[]> = { telecom: [], unicom: [], mobile: [] }
    const others: LineSample[] = []
    for (const { probe, results } of [...rows].sort((a, b) => byName(a.probe, b.probe))) {
      const sample = lineSample(probe, results, now, unavailable)
      const carrier = carrierOf(probe)
      if (carrier) lines[carrier].push(sample)
      else others.push(sample)
    }
    const endpoints = targets?.filter(target => target.nodes.some(node => node.server_id === server.id)) ?? []
    const line = {
      server: { id: server.id, name: server.name, online: server.online !== false, region: server.asset_settings?.region ?? '', group: server.asset_settings?.group_name ?? '' },
      carriers: lines, others, endpoints,
      reachability: endpoints.length ? worstState(endpoints) : null,
    }
    return { ...line, problems: lineProblems(line) }
  })
}

export type LineOrder = 'problems' | 'name' | 'latency' | 'loss'
export const lineOrders: [LineOrder, string][] = [['problems', '异常在前'], ['name', '按名称'], ['latency', '按延迟'], ['loss', '按丢包']]

const samplesOf = (line: ServerLine) => carriers.flatMap(([carrier]) => line.carriers[carrier])
function averageLatency(line: ServerLine) {
  const values = samplesOf(line).map(sample => sample.latency).filter((value): value is number => value !== null)
  return values.length ? values.reduce((sum, value) => sum + value, 0) / values.length : null
}
function worstLoss(line: ServerLine) {
  const values = samplesOf(line).map(sample => sample.recentLoss).filter((value): value is number => value !== null)
  return values.length ? Math.max(...values) : null
}
// Rows without a measurement go last in either direction.
const measured = (a: number | null, b: number | null, direction: 1 | -1) => a === null ? (b === null ? 0 : 1) : b === null ? -1 : (a - b) * direction

export function sortLines(lines: ServerLine[], order: LineOrder) {
  const name = (a: ServerLine, b: ServerLine) => a.server.name.localeCompare(b.server.name, 'zh-CN') || a.server.id - b.server.id
  return [...lines].sort((a, b) => {
    switch (order) {
      case 'problems': return Number(b.problems.length > 0) - Number(a.problems.length > 0) || name(a, b)
      case 'latency': return measured(averageLatency(a), averageLatency(b), 1) || name(a, b)
      case 'loss': return measured(worstLoss(a), worstLoss(b), -1) || name(a, b)
      default: return name(a, b)
    }
  })
}

/** Rows whose name, group or region contains the query. */
export function matchLines(lines: ServerLine[], query: string) {
  const text = query.trim().toLocaleLowerCase('zh-CN')
  if (!text) return lines
  return lines.filter(({ server }) => [server.name, server.group, server.region].some(value => value.toLocaleLowerCase('zh-CN').includes(text)))
}

/** The headline figures of one carrier across all rows. */
export function carrierFigures(lines: ServerLine[], carrier: Carrier) {
  const samples = lines.flatMap(line => line.carriers[carrier])
  const latencies = samples.map(sample => sample.latency).filter((value): value is number => value !== null).sort((a, b) => a - b)
  const middle = latencies.length / 2
  return {
    configured: samples.length,
    measured: latencies.length,
    median: !latencies.length ? null : latencies.length % 2 ? latencies[Math.floor(middle)] : (latencies[middle - 1] + latencies[middle]) / 2,
    fastest: latencies[0] ?? null,
    slowest: latencies[latencies.length - 1] ?? null,
    lossy: lines.filter(line => line.carriers[carrier].some(sample => sample.recentLoss !== null && sample.recentLoss >= lossThreshold)).length,
  }
}

/** Where each carrier's targets are, for the column heading: "电信 · 广东" when all agree. */
export function carrierHeading(lines: ServerLine[], carrier: Carrier, label: string) {
  const regions = [...new Set(lines.flatMap(line => line.carriers[carrier]).map(sample => sample.probe.monitor?.region?.trim()).filter(Boolean))]
  return regions.length === 1 ? `${label} · ${regions[0]}` : label
}

/** Endpoint counts for the headline figure; endpoints that cannot be checked are left out. */
export function reachabilityFigures(targets: ReachabilityTarget[]) {
  const checked = targets.filter(target => target.state !== 'not_applicable')
  return {
    total: checked.length,
    reachable: checked.filter(target => target.state === 'reachable').length,
    blocked: checked.filter(target => target.state === 'ip_blocked' || target.state === 'port_blocked' || target.state === 'unconfirmed').length,
    degraded: checked.filter(target => target.state === 'partial' || target.state === 'service_down').length,
    unknown: checked.filter(target => target.state === 'unknown').length,
  }
}
