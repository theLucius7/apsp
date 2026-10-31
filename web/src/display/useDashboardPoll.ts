import { useCallback, useEffect, useRef, useState } from 'react'
import { api, ApiError, errorMessage, isTransient } from '../api'
import { readKept, writeKept } from '@/lib/resource-cache'
import { connectionRestoredEvent } from '@/stores/connection'

type Snapshot<T> = { path: string | null; data?: T; error: string; updatedAt: number | null; loading: boolean; denied: boolean }
type Kept<T> = { data: T; at: number }
const empty = <T,>(path: string | null, kept?: Kept<T>): Snapshot<T> => ({ path, data: kept?.data, error: '', updatedAt: kept?.at ?? null, loading: Boolean(path), denied: false })

// Every request has its own identity. Hidden pages abort in-flight reads, and a
// late response from an old route or cleared authorization cannot restore data.
export function useDashboardPoll<T>(path: string | null, interval: number, paused: boolean, identity = '', reconcile?: (previous: T | undefined, incoming: T) => T) {
  const key = path === null ? null : `${identity}\n${path}`
  const kept = () => key === null ? undefined : readKept<Kept<T>>(key)
  const [snapshot, setSnapshot] = useState<Snapshot<T>>(() => empty(key, kept()))
  const trigger = useRef<() => void>(() => {})
  const reset = useRef<() => void>(() => {})
  const reload = useCallback(() => trigger.current(), [])
  const clear = useCallback(() => reset.current(), [])
  useEffect(() => {
    let active = true, controller: AbortController | undefined, timeout: number | undefined, losses = 0
    const cancel = () => { window.clearTimeout(timeout); const pending = controller; controller = undefined; pending?.abort() }
    const update = (patch: Partial<Snapshot<T>> | ((current: Snapshot<T>) => Partial<Snapshot<T>>)) => setSnapshot(current => {
      const previous = current.path === key ? current : empty<T>(key, kept())
      const changes = typeof patch === 'function' ? patch(previous) : patch
      if (previous === current && (Object.keys(changes) as (keyof Snapshot<T>)[]).every(field => previous[field] === changes[field])) return current
      return { ...previous, ...changes }
    })
    update({ loading: Boolean(path) && !paused })
    const load = async (manual = false) => {
      if (!path || controller || document.visibilityState !== 'visible' || (paused && !manual)) return
      const request = new AbortController(); controller = request
      // Only a first read shows progress; polls change nothing unless the answer does.
      if (manual) update({ loading: true })
      let timedOut = false
      timeout = window.setTimeout(() => { timedOut = true; request.abort() }, 12_000)
      try {
        const result = await api<T>(path, 'GET', undefined, request.signal)
        if (active && controller === request && !request.signal.aborted) {
          losses = 0
          if (key !== null) writeKept(key, { data: result, at: Date.now() })
          update(current => ({ data: reconcile ? reconcile(current.data, result) : result, updatedAt: Date.now(), error: '', denied: false }))
        }
      } catch (reason) {
        if (active && controller === request && (!request.signal.aborted || timedOut)) {
          const denied = reason instanceof ApiError && [401, 403, 404].includes(reason.status)
          // One lost read during a poll is not news; the next read usually succeeds.
          losses += 1
          const quiet = !timedOut && isTransient(reason) && !manual && losses === 1
          update(current => quiet && current.data !== undefined ? {} : { error: timedOut ? '读取超过 12 秒，保留上次快照，请重试。' : errorMessage(reason), denied, ...(denied ? { data: undefined, updatedAt: null } : {}) })
        }
      } finally {
        if (controller === request) { window.clearTimeout(timeout); controller = undefined; if (active) update({ loading: false }) }
      }
    }
    const refresh = () => {
      if (document.visibilityState === 'visible') void load()
      else { cancel(); update({ loading: false }) }
    }
    trigger.current = () => { void load(true) }
    reset.current = () => { cancel(); if (key !== null) writeKept(key, undefined); update({ data: undefined, updatedAt: null, loading: false }) }
    if (!paused) void load()
    const timer = paused || !path ? undefined : window.setInterval(refresh, interval)
    document.addEventListener('visibilitychange', refresh)
    window.addEventListener('online', refresh)
    window.addEventListener(connectionRestoredEvent, refresh)
    return () => {
      active = false; cancel(); trigger.current = () => {}; reset.current = () => {}; window.clearInterval(timer)
      document.removeEventListener('visibilitychange', refresh); window.removeEventListener('online', refresh); window.removeEventListener(connectionRestoredEvent, refresh)
    }
  }, [path, key, interval, paused, reconcile])
  return { ...(snapshot.path === key ? snapshot : empty<T>(key, kept())), reload, clear }
}
