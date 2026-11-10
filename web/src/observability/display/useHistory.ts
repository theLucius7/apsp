import { useCallback, useEffect, useRef, useState } from 'react'
import { api, ApiError, errorMessage } from '../../api'
import type { Sample } from './data'
import { historyPollInterval, windowMinutes } from './history'
import type { AggregateHistory, HistoryWindow } from './history'

type Snapshot = { key: string; samples: Sample[]; aggregate?: AggregateHistory; error: string; loading: boolean; denied: boolean }
const empty = (key: string): Snapshot => ({ key, samples: [], error: '', loading: true, denied: false })

/** `base` is the server's endpoint prefix: the board's or the console's. */
export function useHistory(base: string, window: HistoryWindow, modern: boolean) {
  const key = `${base}/${window}/${modern}`
  const [snapshot, setSnapshot] = useState<Snapshot>(() => empty(key))
  const trigger = useRef<() => void>(() => {})
  const reload = useCallback(() => trigger.current(), [])
  useEffect(() => {
    let active = true, controller: AbortController | undefined, timeout: number | undefined
    let saved: Sample[] = [], lastFullRead = 0, nextRead = 0, interval = 15_000
    const update = (patch: Partial<Snapshot>) => setSnapshot(current => ({ ...(current.key === key ? current : empty(key)), ...patch }))
    const cancel = () => { globalThis.clearTimeout(timeout); const request = controller; controller = undefined; request?.abort() }
    setSnapshot(empty(key))
    const load = async (manual = false) => {
      if (controller || document.visibilityState !== 'visible' || (!manual && Date.now() < nextRead)) return
      const request = new AbortController(); controller = request
      update({ loading: true })
      const since = Date.now() - windowMinutes(window) * 60_000
      const newest = saved[saved.length - 1]?.sampled_at
      const fullRead = Date.now() - lastFullRead >= 60_000
      let timedOut = false
      timeout = globalThis.setTimeout(() => { timedOut = true; request.abort() }, 12_000)
      try {
        if (modern) {
          const result = await api<AggregateHistory>(`${base}/history?window=${window}`, 'GET', undefined, request.signal)
          if (!active || controller !== request || request.signal.aborted) return
          if (result.window !== window) throw new Error('返回的历史窗口不匹配，请重试。')
          interval = historyPollInterval(result.bucket_ms)
          update({ aggregate: result, error: '', denied: false })
        } else {
          // Periodically include delayed uploads while preserving the legacy raw endpoint.
          const result = await api<Sample[]>(`${base}/metrics?since=${Math.max(since, !fullRead && newest ? newest - 120_000 : since)}`, 'GET', undefined, request.signal)
          if (!active || controller !== request || request.signal.aborted) return
          const merged = new Map([...saved, ...result].map(sample => [sample.id, sample]))
          saved = [...merged.values()].filter(sample => sample.sampled_at >= since && sample.sampled_at <= Date.now() + 60_000)
            .sort((a, b) => a.sampled_at - b.sampled_at).slice(-7200)
          if (fullRead) lastFullRead = Date.now()
          update({ samples: saved, error: '', denied: false })
        }
        nextRead = Date.now() + interval
      } catch (reason) {
        if (active && controller === request && (!request.signal.aborted || timedOut)) {
          const denied = reason instanceof ApiError && [401, 403, 404].includes(reason.status)
          if (denied) saved = []
          update({ error: timedOut ? '历史读取超过 12 秒，请重试。' : errorMessage(reason), denied, ...(denied ? { samples: [], aggregate: undefined } : {}) })
          nextRead = Date.now() + 15_000
        }
      } finally {
        if (controller === request) { globalThis.clearTimeout(timeout); controller = undefined; if (active) update({ loading: false }) }
      }
    }
    const refresh = () => {
      if (document.visibilityState === 'visible') { nextRead = 0; void load() }
      else { cancel(); update({ loading: false }) }
    }
    trigger.current = () => { void load(true) }
    void load()
    const timer = globalThis.setInterval(() => { void load() }, 15_000)
    document.addEventListener('visibilitychange', refresh)
    return () => { active = false; cancel(); globalThis.clearInterval(timer); document.removeEventListener('visibilitychange', refresh); trigger.current = () => {} }
  }, [key, base, window, modern])
  return { ...(snapshot.key === key ? snapshot : empty(key)), reload }
}
