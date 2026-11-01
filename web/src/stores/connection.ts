import { create } from 'zustand'

export type ConnectionStatus = 'online' | 'reconnecting' | 'offline'

type ConnectionState = {
  status: ConnectionStatus
  /** When the panel last answered a request, in milliseconds. */
  lastContact: number | null
  /** When the current disruption was confirmed; null while connected. */
  since: number | null
}

// A single failed request says little: browsers drop sockets when a laptop wakes
// or a proxy changes route. The failure starts a quiet probe, and only a failed
// probe tells the administrator; a brief loss never reaches the screen.
const probeDelays = [1500, 3000, 6000, 12_000, 15_000]
const probeTimeout = 10_000
const probePath = '/api/dashboard/access'
export const connectionRestoredEvent = 'apsp:connection-restored'

export const useConnectionStore = create<ConnectionState>()(() => ({ status: 'online', lastContact: null, since: null }))

let failed = false
let attempts = 0
let probe: ReturnType<typeof setTimeout> | undefined
let probing = false
let knownAuthenticated: () => boolean = () => false

/** Gateways answer with these while the panel restarts; they say nothing about the request. */
const gatewayStatuses = new Set([502, 503, 504, 520, 521, 522, 523, 524, 525, 526, 527, 530])
export const isGatewayStatus = (status: number) => gatewayStatuses.has(status)

/** Any answer from the panel itself ends a disruption and asks stale reads to refresh. */
export function panelAnswered() {
  const state = useConnectionStore.getState()
  const recovered = failed
  failed = false
  attempts = 0
  clearTimeout(probe)
  probe = undefined
  useConnectionStore.setState({ status: 'online', since: null, lastContact: Date.now() })
  if (recovered || state.status !== 'online') globalThis.dispatchEvent?.(new Event(connectionRestoredEvent))
}

function schedule(delay: number) {
  if (probe !== undefined) return
  probe = setTimeout(() => {
    probe = undefined
    void run()
  }, delay)
}

async function run() {
  if (probing) return
  probing = true
  const controller = new AbortController()
  const timer = setTimeout(() => controller.abort(), probeTimeout)
  try {
    const response = await fetch(probePath, { credentials: 'same-origin', cache: 'no-store', signal: controller.signal })
    if (isGatewayStatus(response.status)) throw new Error('gateway')
    const access = await response.json().catch(() => null) as { authenticated?: boolean } | null
    panelAnswered()
    // The session may have ended during the outage; the router returns to sign-in.
    if (access?.authenticated === false && knownAuthenticated()) window.dispatchEvent(new Event('apsp:unauthorized'))
  } catch {
    const state = useConnectionStore.getState()
    const status: ConnectionStatus = navigator.onLine === false ? 'offline' : 'reconnecting'
    useConnectionStore.setState({ status, since: state.since ?? Date.now() })
    attempts = Math.min(attempts + 1, probeDelays.length - 1)
    schedule(probeDelays[attempts])
  } finally {
    clearTimeout(timer)
    probing = false
  }
}

/** A read or write could not reach the panel: start probing without telling anyone yet. */
export function panelUnreachable() {
  failed = true
  schedule(probeDelays[attempts])
}

/** Tries again at once, for the retry button and for the browser reporting a network. */
export function retryConnection() {
  clearTimeout(probe)
  probe = undefined
  void run()
}

/** Listens for the browser's own network events; called once at startup. */
export function installConnectionMonitor(authenticated: () => boolean) {
  knownAuthenticated = authenticated
  window.addEventListener('offline', () => {
    failed = true
    useConnectionStore.setState(state => ({ status: 'offline', since: state.since ?? Date.now() }))
    schedule(probeDelays[attempts])
  })
  window.addEventListener('online', retryConnection)
}
