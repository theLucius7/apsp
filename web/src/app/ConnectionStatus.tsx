import { useEffect, useRef, useState } from 'react'
import { useI18n } from '../i18n'
import { cn } from '@/lib/utils'
import { retryConnection, useConnectionStore } from '@/stores/connection'
import { Button } from '@/components/ui/button'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'

const clock = (at: number) => new Date(at).toLocaleTimeString('zh-CN', { hour12: false })

/** The state of the connection to the panel, always in the same place. */
export function ConnectionStatus({ compact = false }: { compact?: boolean }) {
  const { t } = useI18n()
  const status = useConnectionStore(state => state.status)
  const label = t(status === 'online' ? '已连接面板' : status === 'offline' ? '网络已断开' : '正在重新连接')
  const dot = <span data-slot="connection-dot" data-status={status} className={cn('size-2 shrink-0 rounded-full', status === 'online' ? 'bg-success' : 'bg-warning animate-pulse')} aria-hidden="true" />
  if (compact) return <Tooltip><TooltipTrigger asChild>
    <span data-slot="connection-status" role="status" aria-label={label} tabIndex={0} className="focus-visible:ring-ring/50 grid size-8 shrink-0 place-items-center rounded-md outline-none focus-visible:ring-3">{dot}</span>
  </TooltipTrigger><TooltipContent side="right">{label}</TooltipContent></Tooltip>
  return <div data-slot="connection-status" role="status" className="text-muted-foreground flex h-7 items-center gap-2 px-2 text-xs">{dot}<span className="truncate">{label}</span></div>
}

/**
 * Says once, over the page and without moving it, that the connection is gone and
 * what that means. Pages keep showing what they last read. It appears only after a
 * probe confirmed the loss, and says so again briefly when the panel is back.
 */
export function ConnectionNotice() {
  const { t } = useI18n()
  const status = useConnectionStore(state => state.status)
  const lastContact = useConnectionStore(state => state.lastContact)
  const [recovered, setRecovered] = useState(false)
  const previous = useRef(status)
  useEffect(() => {
    const was = previous.current
    previous.current = status
    if (was === 'online' || status !== 'online') { setRecovered(false); return }
    setRecovered(true)
    const timer = window.setTimeout(() => setRecovered(false), 2500)
    return () => window.clearTimeout(timer)
  }, [status])
  if (status === 'online' && !recovered) return null
  if (status === 'online') return <div data-slot="connection-notice" data-state="recovered" role="status" className="pointer-events-none absolute top-2 left-1/2 z-40 max-w-[calc(100%-1.5rem)] -translate-x-1/2">
    <div className="bg-popover text-popover-foreground ring-foreground/10 flex items-center gap-2 rounded-full px-3 py-1.5 text-xs shadow-md ring-1">
      <span className="bg-success size-2 rounded-full" aria-hidden="true" />{t('已恢复连接')}
    </div>
  </div>
  return <div data-slot="connection-notice" data-state="lost" role="status" className="absolute top-2 left-1/2 z-40 max-w-[calc(100%-1.5rem)] -translate-x-1/2">
    <div className="bg-popover text-popover-foreground ring-foreground/10 flex items-center gap-2.5 rounded-full py-1 pr-1 pl-3 text-xs shadow-md ring-1">
      <span className="bg-warning size-2 shrink-0 animate-pulse rounded-full" aria-hidden="true" />
      <span className="min-w-0">{t(status === 'offline' ? '网络已断开' : '与面板的连接中断，正在重试')}{lastContact ? ` · ${t('显示的是')} ${clock(lastContact)} ${t('的数据')}` : ''}</span>
      <Button variant="ghost" size="xs" className="rounded-full" onClick={retryConnection}>{t('立即重试')}</Button>
    </div>
  </div>
}
