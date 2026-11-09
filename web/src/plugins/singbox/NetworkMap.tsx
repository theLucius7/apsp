import { useEffect, useMemo, useRef, useState } from 'react'
import type { PluginServer, Server } from '../../types'
import { installationView } from './Runtimes'
import { cn } from '@/lib/utils'

type Tone = 'good' | 'bad' | 'warm' | 'neutral'
type Placed = { id: number; name: string; group: string; x: number; y: number; r: number; angle: number; tone: Tone; enabled: boolean; online: boolean; state: string }

const fill: Record<Tone, string> = { good: 'var(--success)', bad: 'var(--destructive)', warm: 'var(--warning)', neutral: 'var(--card)' }
const height = 400

/**
 * The proxy network as a drawing: the panel at the centre, each vendor on the
 * inner ring and every server on the outer ring, in its vendor's sector. Colour
 * says what sing-box does there, size how much traffic it carried this cycle, a
 * red ring that the agent is offline. Every server is a link to its plugins tab.
 * Static SVG laid out once per data change; only the centre's pulse animates.
 */
export default function NetworkMap({ plugins, servers = [] }: { plugins: PluginServer[]; servers?: Server[] }) {
  const host = useRef<HTMLDivElement>(null)
  const [width, setWidth] = useState(880)
  useEffect(() => {
    const observer = new ResizeObserver(entries => { const size = entries[0]?.contentRect.width; if (size) setWidth(Math.max(560, Math.round(size))) })
    if (host.current) observer.observe(host.current)
    return () => observer.disconnect()
  }, [])
  const layout = useMemo(() => {
    // Every server sing-box knows about is drawn; the server list adds its vendor and traffic when readable.
    const byId = new Map(servers.map(server => [server.id, server]))
    const entries = plugins.map(plugin => {
      const server = byId.get(plugin.id)
      return { plugin, server, view: installationView(plugin), group: server?.asset_settings?.group_name || '未分组', traffic: Number(server?.traffic?.used ?? 0) || 0 }
    })
    const groups = [...new Set(entries.map(entry => entry.group))].map(name => ({ name, members: entries.filter(entry => entry.group === name) }))
      .sort((left, right) => right.members.length - left.members.length || left.name.localeCompare(right.name, 'zh-CN'))
    const cx = width / 2, cy = height / 2
    const outer = { rx: width / 2 - 120, ry: height / 2 - 46 }, inner = { rx: outer.rx * 0.46, ry: outer.ry * 0.5 }
    const peak = Math.max(1, ...entries.map(entry => entry.traffic))
    const total = Math.max(1, entries.length)
    let cursor = -Math.PI / 2
    const anchors: { name: string; x: number; y: number; count: number }[] = []
    const nodes: Placed[] = []
    for (const group of groups) {
      const span = 2 * Math.PI * group.members.length / total
      const middle = cursor + span / 2
      anchors.push({ name: group.name, x: cx + inner.rx * Math.cos(middle), y: cy + inner.ry * Math.sin(middle), count: group.members.length })
      group.members.forEach((entry, index) => {
        const angle = cursor + span * (index + 0.5) / group.members.length
        nodes.push({ id: entry.plugin.id, name: entry.plugin.name, group: group.name, angle,
          x: cx + outer.rx * Math.cos(angle), y: cy + outer.ry * Math.sin(angle),
          r: 4.5 + 6 * Math.sqrt(entry.traffic / peak), tone: entry.view.tone, enabled: entry.plugin.enabled,
          // A server still waiting for enrollment is not "offline".
          online: entry.plugin.online || (entry.server ? !entry.server.device_public_key : false), state: entry.view.label })
      })
      cursor += span
    }
    return { cx, cy, anchors, nodes }
  }, [servers, plugins, width])
  const { cx, cy, anchors, nodes } = layout
  const anchor = (group: string) => anchors.find(entry => entry.name === group)!
  return <div ref={host} data-slot="network-map" className="relative min-w-0">
    <svg viewBox={`0 0 ${width} ${height}`} className="block h-auto w-full select-none" role="group" aria-label="代理网络示意图">
      <defs>
        <radialGradient id="network-core"><stop offset="0" stopColor="var(--primary)" /><stop offset="1" stopColor="color-mix(in oklch, var(--primary), var(--foreground) 30%)" /></radialGradient>
        <radialGradient id="network-halo"><stop offset="0" stopColor="var(--primary)" stopOpacity=".18" /><stop offset="1" stopColor="var(--primary)" stopOpacity="0" /></radialGradient>
      </defs>
      {/* Orbits: the two rings the network is laid out on. */}
      <ellipse cx={cx} cy={cy} rx={width / 2 - 120} ry={height / 2 - 46} fill="none" stroke="var(--border)" strokeDasharray="2 6" />
      <ellipse cx={cx} cy={cy} rx={(width / 2 - 120) * 0.46} ry={(height / 2 - 46) * 0.5} fill="none" stroke="var(--border)" strokeDasharray="2 6" />
      <circle cx={cx} cy={cy} r="120" fill="url(#network-halo)" />
      {anchors.map(group => <path key={group.name} d={`M${cx},${cy} Q${(cx + group.x) / 2},${cy} ${group.x},${group.y}`} fill="none" stroke="var(--primary)" strokeOpacity=".28" strokeWidth="1.4" />)}
      {nodes.map(node => { const group = anchor(node.group); return <path key={node.id} d={`M${group.x},${group.y} Q${(group.x + node.x) / 2 + (node.y - group.y) * 0.08},${(group.y + node.y) / 2 - (node.x - group.x) * 0.08} ${node.x},${node.y}`}
        fill="none" stroke={!node.online ? 'var(--destructive)' : node.enabled ? 'var(--primary)' : 'var(--muted-foreground)'} strokeOpacity={node.enabled ? 0.35 : 0.18} strokeWidth="1" strokeDasharray={node.online ? undefined : '3 3'} /> })}
      {anchors.map(group => <g key={group.name}>
        <circle cx={group.x} cy={group.y} r="3" fill="var(--primary)" fillOpacity=".55" />
        <text x={group.x} y={group.y - 9} textAnchor="middle" className="fill-foreground text-[11px] font-medium">{group.name}<tspan className="fill-muted-foreground" dx="4">{group.count}</tspan></text>
      </g>)}
      {/* The panel: everything above reports to it. */}
      <circle cx={cx} cy={cy} r="20" fill="url(#network-core)" />
      <path d={`M${cx - 9},${cy + 4.5}c3.5,0 4.5,-9.5 9,-9.5s5.5,9.5 9,9.5`} fill="none" stroke="var(--primary-foreground)" strokeWidth="1.6" strokeLinecap="round" />
      <text x={cx} y={cy + 38} textAnchor="middle" className="fill-muted-foreground text-[11px]">面板</text>
      {nodes.map(node => {
        const cos = Math.cos(node.angle), side = cos > 0.25 ? 'start' : cos < -0.25 ? 'end' : 'middle'
        const label = { x: node.x + Math.cos(node.angle) * (node.r + 8), y: node.y + Math.sin(node.angle) * (node.r + 8) + (side === 'middle' ? (Math.sin(node.angle) > 0 ? 10 : -2) : 4) }
        return <a key={node.id} href={`#/servers/${node.id}/services`} aria-label={`${node.name}：${node.state}${node.online ? '' : '，Agent 离线'}`} className="group/node outline-none">
          <title>{`${node.name} · ${node.group} · ${node.state}${node.online ? '' : ' · Agent 离线'}`}</title>
          {!node.online && <circle cx={node.x} cy={node.y} r={node.r + 4} fill="none" stroke="var(--destructive)" strokeWidth="1.2" strokeDasharray="2 2" />}
          <circle cx={node.x} cy={node.y} r={node.r + 6} fill="transparent" className="group-hover/node:fill-[color-mix(in_oklch,var(--primary)_12%,transparent)] group-focus-visible/node:stroke-[var(--ring)]" strokeWidth="2" />
          <circle cx={node.x} cy={node.y} r={node.r} fill={fill[node.tone]} stroke={node.tone === 'neutral' ? 'var(--muted-foreground)' : 'var(--card)'} strokeWidth={node.tone === 'neutral' ? 1.2 : 1.5} />
          <text x={label.x} y={label.y} textAnchor={side} className={cn('text-[11px] transition-colors group-hover/node:fill-foreground', node.enabled ? 'fill-foreground' : 'fill-muted-foreground')}>{node.name}</text>
        </a>
      })}
    </svg>
    {/* The panel's pulse is an HTML layer so the compositor animates it without repainting the drawing. */}
    <span aria-hidden="true" className="bg-primary/15 pointer-events-none absolute size-[60px] -translate-x-1/2 -translate-y-1/2 rounded-full motion-safe:animate-[network-pulse_3.6s_ease-in-out_infinite]" style={{ left: '50%', top: `${cy / height * 100}%` }} />
  </div>
}
