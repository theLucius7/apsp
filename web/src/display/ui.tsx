import type { ReactNode } from 'react'
import type { LucideIcon } from 'lucide-react'
import { CircleAlertIcon, ServerIcon, TriangleAlertIcon } from 'lucide-react'
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

/** Card surface used for every dashboard section; the same as the console's panels. */
export const surface = 'bg-card text-card-foreground shadow-soft rounded-xl border'

/** A board card that opens something: lifts and tints its border on hover, glows on focus. */
export const cardLink = 'group/card hover:border-[color-mix(in_oklch,var(--primary)_35%,var(--border))] hover:shadow-lifted focus-visible:shadow-focus relative flex w-full min-w-0 flex-col overflow-hidden text-left outline-none transition-[border-color,box-shadow,translate] duration-200 hover:-translate-y-0.5 motion-reduce:hover:translate-y-0'

const stripColumns: Record<number, string> = { 2: 'lg:grid-cols-2', 3: 'lg:grid-cols-3', 4: 'lg:grid-cols-4', 5: 'lg:grid-cols-5' }

/** The row of headline figures above a board: one card, divided into cells; two per row below the large breakpoint. */
export function StatStrip({ label, columns, children }: { label: string; columns: 2 | 3 | 4 | 5; children: ReactNode }) {
  return <section aria-label={label} className={cn(surface, 'grid overflow-hidden [&>*]:border-border max-lg:grid-cols-2 max-lg:[&>*:nth-child(even)]:border-l max-lg:[&>*:not(:nth-last-child(-n+2))]:border-b max-lg:[&>*:last-child:nth-child(odd)]:col-span-full lg:[&>*+*]:border-l', stripColumns[columns])}>{children}</section>
}

/** One headline figure of a StatStrip, with a short line of context under it. */
export function Stat({ icon: Icon = ServerIcon, label, value, unit, children, tone, title }: { icon?: LucideIcon; label: string; value: ReactNode; unit?: string; children: ReactNode; tone?: string; title?: string }) {
  return <div data-slot="overview-item" className="min-w-0 border-border px-4 py-2.5 max-sm:px-3" title={title}>
    <div data-slot="overview-label" className="text-muted-foreground mb-0.5 flex items-center justify-between gap-2 text-xs">
      <span>{label}</span>
      <Icon className="size-3.5 opacity-70" aria-hidden="true" />
    </div>
    <div data-slot="overview-value" className="flex min-w-0 items-baseline gap-1.5">
      <strong className={cn('truncate text-xl leading-tight font-semibold tabular-nums', tone && toneText[tone])}>{value}</strong>
      {unit && <b className="text-muted-foreground shrink-0 text-xs font-medium">{unit}</b>}
    </div>
    <div className="text-muted-foreground mt-0.5 flex min-h-4 items-center gap-2 overflow-hidden text-xs whitespace-nowrap *:truncate">{children}</div>
  </div>
}

export function SectionHeading({ icon: Icon, title, children }: { icon: LucideIcon; title: string; children?: ReactNode }) {
  return <div className="flex items-center justify-between gap-3 max-sm:flex-col max-sm:items-stretch">
    <h2 className="flex items-center gap-2 text-sm font-semibold"><Icon className="text-primary size-4" aria-hidden="true" />{title}</h2>
    {children}
  </div>
}

export function DisplayNotice({ tone = 'warning', children, onRetry, retryLabel = '重试', role }: { tone?: 'warning' | 'error'; children: ReactNode; onRetry?: () => void; retryLabel?: string; role?: 'alert' | 'status' }) {
  const Icon = tone === 'error' ? CircleAlertIcon : TriangleAlertIcon
  return <div data-slot="display-notice" role={role ?? (tone === 'error' ? 'alert' : undefined)} className={cn('flex items-start gap-2.5 rounded-xl border px-3.5 py-3 text-sm wrap-anywhere', tone === 'error' ? 'border-destructive/30 bg-destructive/5 text-destructive' : 'border-warning/30 bg-warning/5 text-warning')}>
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
