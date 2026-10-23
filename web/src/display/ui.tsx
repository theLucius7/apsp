import type { ReactNode } from 'react'
import type { LucideIcon } from 'lucide-react'
import { CircleAlertIcon, TriangleAlertIcon } from 'lucide-react'
import { m } from 'motion/react'
import { cn } from '@/lib/utils'
import { transition } from '@/lib/motion'
import { Button } from '@/components/ui/button'

// Building blocks of the public dashboard. They read the same design tokens as
// the administrator console; the dashboard only chooses a roomier layout.

export type Tone = 'good' | 'warning' | 'danger' | 'muted' | 'info'
export const toneText: Record<string, string> = { good: 'text-success', warning: 'text-warning', danger: 'text-destructive', muted: 'text-muted-foreground', info: 'text-info' }
export const toneFill: Record<string, string> = { good: 'bg-success', warning: 'bg-warning', danger: 'bg-destructive', muted: 'bg-muted-foreground', info: 'bg-info' }

/** Probe quality grades, from probeTone(). */
export const qualityText: Record<string, string> = { good: 'text-success', fair: 'text-info', warning: 'text-warning', poor: 'text-chart-5', danger: 'text-destructive', empty: 'text-muted-foreground' }
export const qualityFill: Record<string, string> = { good: 'bg-success', fair: 'bg-info', warning: 'bg-warning', poor: 'bg-chart-5', danger: 'bg-destructive', empty: 'bg-muted-foreground/20' }

/** Translucent card surface used for every dashboard section. */
export const surface = 'bg-card/90 text-card-foreground ring-foreground/10 rounded-xl ring-1 shadow-xs supports-backdrop-filter:backdrop-blur-md'

export function SectionHeading({ icon: Icon, title, children }: { icon: LucideIcon; title: string; children?: ReactNode }) {
  return <div className="flex items-center justify-between gap-3 max-sm:flex-col max-sm:items-stretch">
    <h2 className="flex items-center gap-2 text-sm font-semibold"><Icon className="text-primary size-4" aria-hidden="true" />{title}</h2>
    {children}
  </div>
}

export function DisplayNotice({ tone = 'warning', children, onRetry, retryLabel = '重试', role }: { tone?: 'warning' | 'error'; children: ReactNode; onRetry?: () => void; retryLabel?: string; role?: 'alert' | 'status' }) {
  const Icon = tone === 'error' ? CircleAlertIcon : TriangleAlertIcon
  return <div data-slot="display-notice" role={role ?? (tone === 'error' ? 'alert' : undefined)} className={cn('flex items-start gap-2.5 rounded-lg border px-3.5 py-3 text-sm wrap-anywhere', tone === 'error' ? 'border-destructive/30 bg-destructive/5 text-destructive' : 'border-warning/30 bg-warning/5 text-warning')}>
    <Icon className="mt-0.5 size-4 shrink-0" aria-hidden="true" />
    <span className="flex-1">{children}</span>
    {onRetry && <Button variant="link" size="xs" className="h-auto p-0 text-current underline" onClick={onRetry}>{retryLabel}</Button>}
  </div>
}

/** Thin usage bar; the colour follows the reading's tone. */
export function Track({ value, tone, label }: { value: number | null | undefined; tone: string; label?: string }) {
  return <span className="bg-foreground/10 block h-1 w-full overflow-hidden rounded-full" role={label ? 'img' : undefined} aria-label={label} aria-hidden={label ? undefined : true}>
    <m.span className={cn('block h-full rounded-full', toneFill[tone])} initial={false} animate={{ width: `${Math.min(100, Math.max(0, value ?? 0))}%` }} transition={transition.slow} />
  </span>
}

export function StatusPill({ tone, children }: { tone: string; children: ReactNode }) {
  return <span data-slot="status" className={cn('inline-block rounded-full border border-current px-2 py-0.5 text-xs leading-tight', toneText[tone])}>{children}</span>
}
