import { requestReauthentication } from './lib/reauthentication-request'
import { isGatewayStatus, panelAnswered, panelUnreachable } from './stores/connection'

export class ApiError extends Error {
  constructor(public status: number, message: string, public transient = false) { super(message) }
}
/** The request never got an answer from the panel. */
export class ConnectionError extends Error {}

// A read with no answer must end: pollers wait for the previous read, so one hung
// socket would otherwise freeze the page for good. Writes have no limit because a
// slow server may still be applying them.
const readTimeout = 20_000

export async function api<T>(path: string, method = 'GET', body?: unknown, signal?: AbortSignal, adminSession = true, attemptedReauthentication = false): Promise<T> {
  const reading = method === 'GET'
  const limit = reading ? new AbortController() : undefined
  let timedOut = false
  const timer = limit ? setTimeout(() => { timedOut = true; limit.abort() }, readTimeout) : undefined
  const forward = () => limit?.abort()
  if (limit && signal) {
    if (signal.aborted) limit.abort()
    else signal.addEventListener('abort', forward, { once: true })
  }
  let response: Response
  let text: string
  try {
    response = await fetch(path, { method, credentials: 'same-origin', cache: 'no-store', signal: limit?.signal ?? signal,
      headers: body === undefined ? undefined : { 'Content-Type': 'application/json' },
      body: body === undefined ? undefined : JSON.stringify(body),
    })
    text = await response.text()
  } catch (error) {
    if (!timedOut && error instanceof DOMException && error.name === 'AbortError') throw error
    panelUnreachable()
    throw new ConnectionError(timedOut ? '面板响应超时，请检查网络后重试。' : '无法连接面板，请检查网络后重试。')
  } finally {
    clearTimeout(timer)
    signal?.removeEventListener('abort', forward)
  }
  if (!response.ok) {
    let message = `请求未完成（${response.status}），请稍后重试。`
    let fromPanel = false
    try { const value = JSON.parse(text); if (typeof value.error === 'string') { message = value.error; fromPanel = true } } catch { /* Framework responses may be plain text. */ }
    // A gateway's error page means the panel is restarting, not that this request failed.
    if (isGatewayStatus(response.status) && !fromPanel) {
      panelUnreachable()
      throw new ApiError(response.status, '面板暂时无法访问，请稍后重试。', true)
    }
    panelAnswered()
    if (response.status === 403 && adminSession && !attemptedReauthentication && message.includes('请先再次验证管理员')) {
      await requestReauthentication()
      return api<T>(path, method, body, signal, adminSession, true)
    }
    if (response.status === 401 && adminSession && path !== '/api/login' && !path.startsWith('/api/login/passkey/')) window.dispatchEvent(new Event('apsp:unauthorized'))
    throw new ApiError(response.status, message)
  }
  panelAnswered()
  if (response.status === 204) return undefined as T
  return JSON.parse(text) as T
}
export const errorMessage = (error: unknown) => error instanceof Error ? error.message : '操作未完成，请重试。'
/** Failures that say nothing about the request itself; the shell reports them once. */
export const isTransient = (error: unknown) => error instanceof ConnectionError || (error instanceof ApiError && error.transient)
