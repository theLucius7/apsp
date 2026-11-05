import { useCallback, useEffect, useRef, useState } from 'react'
import { api, ApiError, errorMessage, isTransient } from './api'
import { readKept, writeKept } from '@/lib/resource-cache'
import { connectionRestoredEvent } from '@/stores/connection'

export const resourceRefreshingMessage = '相关信息正在刷新或刷新失败，请成功刷新后再提交；当前草稿已保留。'
/**
 * Poll interval for settings, catalogues and the signed-in administrator: they
 * change only through forms that reload after saving, so a quick poll would just
 * repeat the same answer. Live state (servers, usage, jobs) keeps the default.
 */
export const settingsPoll = 30_000
// A first read that loses its connection tries again before saying anything.
const firstReadRetries = [1000, 3000]

export type ResourceOptions = {
  /** Show the previous reading at once when the page opens again. Lists only: forms must start from a confirmed read. */
  keep?: boolean
}

/**
 * Reads one endpoint and keeps it current. Background polls change nothing on
 * screen unless the data changed. A read invalidates the confirmed snapshot only
 * when it replaces what is known: a manual reload, a new path, or a failure. See
 * ADR 0102 for why a poll in flight no longer blocks writes.
 */
export function useResource<T>(path: string | null, poll = 5000, options: ResourceOptions = {}) {
  const keep = options.keep === true
  const [initial] = useState(() => keep && path ? readKept<T>(path) : undefined)
  const [data, setData] = useState<T | undefined>(initial)
  const [error, setError] = useState('')
  const [loading, setLoading] = useState(initial === undefined)
  const [revision, setRevision] = useState(0)
  const [availability, setAvailability] = useState({ ready: false })
  const previousPath = useRef<string | null>(initial === undefined ? null : path)
  const currentPath = useRef(path)
  currentPath.current = path
  const generation = useRef(0)
  const snapshot = useRef<{ path: string | null; valid: boolean; value?: T }>({ path: initial === undefined ? null : path, valid: false, value: initial })
  // After the first answer of any kind, polls stay quiet whatever they return.
  const settled = useRef(false)
  const reload = useCallback(() => {
    ++generation.current
    snapshot.current.valid = false
    setAvailability(current => current.ready ? { ready: false } : current); setLoading(true)
    setRevision(value => value + 1)
  }, [])
  const isCurrent = useCallback(() => currentPath.current === path && snapshot.current.valid && snapshot.current.path === path, [path])
  const getCurrent = useCallback(() => isCurrent() ? snapshot.current.value : undefined, [isCurrent])
  useEffect(() => {
    if (!path) { snapshot.current = { path: null, valid: false }; setData(undefined); setAvailability({ ready: false }); setLoading(false); return }
    if (previousPath.current !== path) {
      const kept = keep ? readKept<T>(path) : undefined
      snapshot.current = { path, valid: false, value: kept }
      settled.current = false
      setData(kept); setLoading(kept === undefined); setError('')
    }
    previousPath.current = path
    const controller = new AbortController()
    let active = true
    let sequence = 0
    let pending = false
    let retries = 0
    let retry: number | undefined
    let lastRead = 0
    const load = async (background = false) => {
      if (pending) return
      pending = true
      lastRead = Date.now()
      const current = ++sequence, epoch = ++generation.current
      const live = () => active && current === sequence && epoch === generation.current
      if (!background || !settled.current) { snapshot.current.valid = false; setAvailability(state => state.ready ? { ready: false } : state); setLoading(true) }
      try {
        const result = await api<T>(path, 'GET', undefined, controller.signal)
        if (live()) {
          snapshot.current = { path, valid: true, value: result }
          settled.current = true
          retries = 0
          if (keep) writeKept(path, result)
          setData(previous => JSON.stringify(previous) === JSON.stringify(result) ? previous : result)
          // Unchanged readings cause no render at all.
          setError(''); setAvailability(state => state.ready ? state : { ready: true })
          setLoading(false)
        }
      } catch (reason) {
        if (live()) {
          const transient = isTransient(reason)
          snapshot.current.valid = false
          setAvailability(state => state.ready ? { ready: false } : state)
          if (transient && snapshot.current.value === undefined && retries < firstReadRetries.length) {
            retry = window.setTimeout(() => void load(), firstReadRetries[retries++])
          } else {
            settled.current = true
            // The shell reports a lost connection once; pages keep what they show.
            if (!transient || snapshot.current.value === undefined) setError(errorMessage(reason))
            if (path.startsWith('/api/dashboard/') && reason instanceof ApiError && [401, 403, 404].includes(reason.status)) { snapshot.current.value = undefined; setData(undefined) }
            setLoading(false)
          }
        }
      } finally { pending = false }
    }
    // Coming back to the tab or regaining the panel refreshes now rather than at the next tick.
    const resume = () => { if (document.visibilityState === 'visible' && Date.now() - lastRead > 1000) void load(true) }
    const restored = () => { if (document.visibilityState === 'visible') void load(true) }
    void load(settled.current && snapshot.current.valid)
    const timer = poll ? window.setInterval(() => { if (document.visibilityState === 'visible') void load(true) }, poll) : undefined
    document.addEventListener('visibilitychange', resume)
    window.addEventListener(connectionRestoredEvent, restored)
    return () => {
      active = false; snapshot.current.valid = false; ++generation.current; controller.abort(); window.clearTimeout(retry); window.clearInterval(timer)
      document.removeEventListener('visibilitychange', resume); window.removeEventListener(connectionRestoredEvent, restored)
    }
  }, [path, poll, revision, keep])
  const currentData = previousPath.current === path ? data : undefined
  return { data: currentData, error, loading: loading && currentData === undefined, ready: previousPath.current === path && availability.ready,
    refreshing: loading || !isCurrent(), fresh: currentData !== undefined && currentData !== null && !error && isCurrent(),
    reload, isCurrent, getCurrent }
}

export type ResourceState<T> = ReturnType<typeof useResource<T>>

const sameFlags = <K extends string>(left: Partial<Record<K, unknown>>, right: Partial<Record<K, unknown>>) => {
  const keys = Object.keys(left) as K[]
  return keys.length === Object.keys(right).length && keys.every(key => left[key] === right[key])
}

/**
 * Reads several endpoints in one round and publishes changed data together, so
 * sections that combine them never render a mix of old and new responses.
 * Write guards follow useResource: a manual reload invalidates all members at
 * once and a background round invalidates none. During a reload a member whose
 * response equals the data already shown is current as soon as it arrives; changed
 * data becomes current when the whole round is published. A failed member stays
 * unavailable while the others remain current.
 */
export function useResourceGroup<T extends Record<string, unknown>>(paths: { [K in keyof T]: string }, poll = 5000, options: ResourceOptions = {}): { [K in keyof T]: ResourceState<T[K]> } {
  type Key = keyof T & string
  type Flags = Partial<Record<Key, boolean>>
  const keep = options.keep === true
  const keys = Object.keys(paths) as Key[]
  const signature = keys.map(key => `${key}=${paths[key]}`).join('\n')
  const keptValues = () => {
    const values: Partial<T> = {}
    if (keep) for (const key of keys) { const value = readKept<T[Key]>(paths[key]); if (value !== undefined) values[key] = value }
    return values
  }
  const [initial] = useState(keptValues)
  const hasInitial = Object.keys(initial).length === keys.length
  const [values, setValues] = useState<Partial<T>>(hasInitial ? initial : {})
  const [errors, setErrors] = useState<Partial<Record<Key, string>>>({})
  const [loading, setLoading] = useState(!hasInitial)
  const [ready, setReady] = useState<Flags>({})
  const [revision, setRevision] = useState(0)
  const generation = useRef(0)
  const snapshot = useRef<{ signature: string; valid: Flags; values: Partial<T> }>({ signature: hasInitial ? signature : '', valid: {}, values: hasInitial ? initial : {} })
  const currentSignature = useRef(signature)
  currentSignature.current = signature
  const previousSignature = useRef<string | null>(hasInitial ? signature : null)
  const latestPaths = useRef(paths)
  latestPaths.current = paths
  const settled = useRef(false)
  const reload = useCallback(() => {
    ++generation.current
    snapshot.current.valid = {}
    setReady(current => Object.keys(current).length ? {} : current); setLoading(true)
    setRevision(value => value + 1)
  }, [])
  useEffect(() => {
    if (previousSignature.current !== signature) {
      const kept = keptValues()
      const complete = Object.keys(kept).length === keys.length
      snapshot.current = { signature, valid: {}, values: complete ? kept : {} }
      settled.current = false
      setValues(complete ? kept : {}); setErrors({}); setLoading(!complete)
    }
    previousSignature.current = signature
    const controller = new AbortController()
    let active = true
    let sequence = 0
    let pending = false
    let retries = 0
    let retry: number | undefined
    let lastRead = 0
    const load = async (background = false) => {
      if (pending) return
      pending = true
      lastRead = Date.now()
      const round = ++sequence, epoch = ++generation.current
      const live = () => active && round === sequence && epoch === generation.current
      const quiet = background && settled.current
      if (!quiet) { snapshot.current.valid = {}; setReady(current => Object.keys(current).length ? {} : current); setLoading(true) }
      const requests = keys.map(key => api<unknown>(latestPaths.current[key], 'GET', undefined, controller.signal).then(value => {
        // A response equal to the data shown is current now; changed data waits for the round.
        if (!quiet && live() && Object.hasOwn(snapshot.current.values, key) && JSON.stringify(value) === JSON.stringify(snapshot.current.values[key])) {
          snapshot.current.valid[key] = true
          setReady(current => current[key] ? current : { ...current, [key]: true })
        }
        return value
      }))
      const results = await Promise.allSettled(requests)
      pending = false
      if (!live()) return
      const nextValues: Partial<T> = { ...snapshot.current.values }
      const valid: Flags = {}
      const nextErrors: Partial<Record<Key, string>> = {}
      let waiting = false
      results.forEach((result, index) => {
        const key = keys[index]
        if (result.status === 'fulfilled') {
          nextValues[key] = result.value as T[Key]; valid[key] = true
          if (keep) writeKept(latestPaths.current[key], result.value)
          return
        }
        const transient = isTransient(result.reason)
        const known = Object.hasOwn(nextValues, key)
        if (transient && !known && retries < firstReadRetries.length) waiting = true
        // The shell reports a lost connection once; sections keep what they show.
        else if (!transient || !known) nextErrors[key] = errorMessage(result.reason)
      })
      snapshot.current = { signature, valid, values: nextValues }
      // Unchanged members keep their identity so dependent memoized views stay stable.
      setValues(previous => {
        const merged: Partial<T> = { ...nextValues }
        for (const key of keys) if (JSON.stringify(previous[key]) === JSON.stringify(nextValues[key])) merged[key] = previous[key]
        return keys.every(key => merged[key] === previous[key]) ? previous : merged
      })
      setErrors(current => sameFlags<Key>(current, nextErrors) ? current : nextErrors)
      setReady(current => sameFlags<Key>(current, valid) ? current : { ...valid })
      if (waiting) { retry = window.setTimeout(() => void load(), firstReadRetries[retries++]); return }
      retries = 0
      settled.current = true
      setLoading(false)
    }
    const resume = () => { if (document.visibilityState === 'visible' && Date.now() - lastRead > 1000) void load(true) }
    const restored = () => { if (document.visibilityState === 'visible') void load(true) }
    void load()
    const timer = poll ? window.setInterval(() => { if (document.visibilityState === 'visible') void load(true) }, poll) : undefined
    document.addEventListener('visibilitychange', resume)
    window.addEventListener(connectionRestoredEvent, restored)
    return () => {
      active = false; snapshot.current.valid = {}; ++generation.current; controller.abort(); window.clearTimeout(retry); window.clearInterval(timer)
      document.removeEventListener('visibilitychange', resume); window.removeEventListener(connectionRestoredEvent, restored)
    }
    // Keys and paths are captured through the signature.
  }, [signature, poll, revision, keep])
  const matches = previousSignature.current === signature
  const result = {} as { [K in keyof T]: ResourceState<T[K]> }
  for (const key of keys) {
    const isCurrent = () => currentSignature.current === signature && snapshot.current.signature === signature && snapshot.current.valid[key] === true
    const data = matches ? values[key] as T[Key] | undefined : undefined
    const error = errors[key] ?? ''
    const memberLoading = loading && ready[key] !== true
    result[key] = {
      data, error, loading: memberLoading && data === undefined, ready: matches && ready[key] === true && !error,
      refreshing: memberLoading || !isCurrent(), fresh: data !== undefined && data !== null && !error && isCurrent(),
      reload, isCurrent, getCurrent: () => isCurrent() ? snapshot.current.values[key] as T[Key] : undefined,
    }
  }
  return result
}

/**
 * Reports a click rejected because related reads are still refreshing, instead
 * of ignoring it silently. The action is not deferred: writes and dialogs only
 * use a confirmed snapshot, so the person retries once the refresh completes.
 */
export function useRefreshNotice() {
  const [shownAt, setShownAt] = useState(0)
  useEffect(() => {
    if (!shownAt) return
    const timer = window.setTimeout(() => setShownAt(0), 3000)
    return () => window.clearTimeout(timer)
  }, [shownAt])
  return {
    visible: shownAt !== 0,
    /** Returns true when the action may proceed now; a refresh in flight is reported. */
    allows: (error: string) => {
      if (error === resourceRefreshingMessage) setShownAt(Date.now())
      return !error
    },
  }
}

export function resourceWriteError(...resources: { isCurrent?: () => boolean; fresh?: boolean; error?: string }[]): string {
  const failure = resources.find(resource => resource.error)
  if (failure) return `最新信息读取失败，暂不能修改；草稿已保留。${failure.error}`
  return resources.every(resource => resource.isCurrent ? resource.isCurrent() : resource.fresh === true)
    ? '' : resourceRefreshingMessage
}

export function useAction() {
  const alive = useRef(true)
  const locked = useRef(false)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState('')
  useEffect(() => { alive.current = true; return () => { alive.current = false } }, [])
  const run = async <T,>(task: () => Promise<T>, success?: (value: T) => void) => {
    if (locked.current) return
    locked.current = true
    setBusy(true); setError('')
    try { const result = await task(); if (alive.current) success?.(result) }
    catch (error) { if (alive.current) setError(errorMessage(error)) }
    finally { locked.current = false; if (alive.current) setBusy(false) }
  }
  return { busy, error, run, clearError: () => setError('') }
}
