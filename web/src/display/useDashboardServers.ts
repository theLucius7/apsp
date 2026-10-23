import { useEffect, useMemo, useRef, useState } from 'react'
import type { Server } from '../types'
import { mergeLiveServers, mergeLiveSnapshot } from './live'
import type { LiveSnapshot } from './live'
import { useDashboardPoll } from './useDashboardPoll'

export function useDashboardServers(paused: boolean, id?: number) {
  const [liveCapable, setLiveCapable] = useState(false)
  const metadata = useDashboardPoll<Server[] | Server>(`/api/dashboard/servers${id ? `/${id}` : ''}`, liveCapable ? 30_000 : 5000, paused)
  const rows = useMemo(() => metadata.data ? Array.isArray(metadata.data) ? metadata.data : [metadata.data] : undefined, [metadata.data])
  const modern = liveCapable || Boolean(rows?.some(server => server.telemetry_settings))
  useEffect(() => { if (modern) setLiveCapable(true) }, [modern])
  const live = useDashboardPoll<LiveSnapshot>(modern ? '/api/dashboard/live' : null, 3000, paused, '', mergeLiveSnapshot)
  const scopeChanged = Boolean(rows?.length && live.data && rows.some(server => Boolean(server.public_view) !== live.data!.public_view))
  const reconciled = useRef<LiveSnapshot | undefined>(undefined)
  useEffect(() => {
    if (live.denied) metadata.clear()
  }, [live.denied, metadata.clear])
  useEffect(() => {
    if (!rows || !live.data || reconciled.current === live.data) return
    reconciled.current = live.data
    const ids = new Set(live.data.servers.map(server => server.id))
    const removed = rows.some(server => !ids.has(server.id))
    if (scopeChanged || removed) {
      // Forget metadata and abort older reads before requesting the new visible
      // projection. Reconcile each live snapshot once to avoid refresh loops.
      metadata.clear(); metadata.reload()
      if (scopeChanged && live.data.public_view) window.dispatchEvent(new Event('apsp:unauthorized'))
    } else if (!id && live.data.servers.some(server => !rows.some(row => row.id === server.id))) metadata.reload()
  }, [rows, live.data, scopeChanged, id, metadata.clear, metadata.reload])
  useEffect(() => { if (metadata.denied) live.clear() }, [metadata.denied, live.clear])
  const data = useMemo(() => {
    if (!rows || live.denied || metadata.denied || scopeChanged) return undefined
    return live.data ? mergeLiveServers(rows, live.data) : rows.filter(server => !server.asset_settings?.hidden)
  }, [rows, live.data, live.denied, metadata.denied, scopeChanged])
  const missing = Boolean(id && rows?.length && live.data && !live.data.servers.some(server => server.id === id))
  const hiddenOnly = Boolean(rows?.length && rows.every(server => server.asset_settings?.hidden))
  return { data, modern, hiddenOnly, error: live.error || metadata.error || (missing ? '该服务器已隐藏或不再可见。' : ''), loading: metadata.loading || live.loading,
    updatedAt: modern ? live.updatedAt ?? metadata.updatedAt : metadata.updatedAt,
    reload: () => { metadata.reload(); if (modern) live.reload() } }
}
