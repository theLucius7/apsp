import type { ReactNode } from 'react'
import type { LucideIcon } from 'lucide-react'
import { ServerIcon } from 'lucide-react'
import { cn } from '@/lib/utils'
import { Skeleton } from '@/components/ui/skeleton'

// Building blocks every board shares: the public dashboard, the overview and the
// headline figures above a page's list. They read the console's design tokens.

/** How a reading looks: fine, needs a look, failing, not applicable or plain information. */
export type Tone = 'good' | 'warning' | 'danger' | 'muted' | 'info'
export const toneText: Record<string, string> = { good: 'text-success', warning: 'text-warning', danger: 'text-destructive', muted: 'text-muted-foreground', info: 'text-info' }
export const toneFill: Record<string, string> = { good: 'bg-success', warning: 'bg-warning', danger: 'bg-destructive', muted: 'bg-muted-foreground', info: 'bg-info' }
/** Probe quality grades, from probeTone(): five steps in order; "poor" mixes warning and danger. */
export const qualityText: Record<string, string> = { good: 'text-success', fair: 'text-info', warning: 'text-warning', poor: 'text-[color-mix(in_oklch,var(--warning),var(--destructive))]', danger: 'text-destructive', empty: 'text-muted-foreground' }
export const qualityFill: Record<string, string> = { good: 'bg-success', fair: 'bg-info', warning: 'bg-warning', poor: 'bg-[color-mix(in_oklch,var(--warning),var(--destructive))]', danger: 'bg-destructive', empty: 'bg-muted-foreground/20' }

/** Card surface used for every board section; the same as the console's panels. */
export const surface = 'bg-card text-card-foreground shadow-soft rounded-xl border'

/** A board card that opens something: lifts and tints its border on hover, glows on focus. */
export const cardLink = 'group/card hover:border-[color-mix(in_oklch,var(--primary)_35%,var(--border))] hover:shadow-lifted focus-visible:shadow-focus relative flex w-full min-w-0 flex-col overflow-hidden text-left outline-none transition-[border-color,box-shadow,translate] duration-200 hover:-translate-y-0.5 motion-reduce:hover:translate-y-0'

const stripColumns: Record<number, string> = { 2: 'lg:grid-cols-2', 3: 'lg:grid-cols-3', 4: 'lg:grid-cols-4', 5: 'lg:grid-cols-5' }

/** The row of headline figures above a board: one card, divided into cells; two per row below the large breakpoint. */
export function StatStrip({ label, columns, children }: { label: string; columns: 2 | 3 | 4 | 5; children: ReactNode }) {
  return <section aria-label={label} className={cn(surface, 'grid overflow-hidden [&>*]:border-border max-lg:grid-cols-2 max-lg:[&>*:nth-child(even)]:border-l max-lg:[&>*:not(:nth-last-child(-n+2))]:border-b max-lg:[&>*:last-child:nth-child(odd)]:col-span-full lg:[&>*+*]:border-l', stripColumns[columns])}>{children}</section>
}

/**
 * One headline figure of a StatStrip, with a short line of context under it. Every
 * page's figures use this one cell. While `loading`, the value is a placeholder bar
 * of the same height, so nothing moves when the number arrives.
 */
export function Stat({ icon: Icon = ServerIcon, label, value, unit, children, tone, title, loading = false }: { icon?: LucideIcon; label: string; value: ReactNode; unit?: string; children?: ReactNode; tone?: string; title?: string; loading?: boolean }) {
  return <div data-slot="overview-item" className="min-w-0 border-border px-4 py-2.5 max-sm:px-3" title={title} aria-busy={loading || undefined}>
    <div data-slot="overview-label" className="text-muted-foreground mb-0.5 flex items-center justify-between gap-2 text-xs">
      <span className="truncate">{label}</span>
      <Icon className="size-3.5 shrink-0 opacity-70" aria-hidden="true" />
    </div>
    <div data-slot="overview-value" className="flex min-h-7 min-w-0 items-baseline gap-1.5">
      {loading ? <Skeleton className="mt-1 h-5 w-16" /> : <>
        <strong className={cn('truncate text-xl leading-tight font-semibold tabular-nums', tone && toneText[tone])}>{value}</strong>
        {unit && <b className="text-muted-foreground shrink-0 text-xs font-medium">{unit}</b>}
      </>}
    </div>
    <div className="text-muted-foreground mt-0.5 flex min-h-4 items-center gap-2 overflow-hidden text-xs whitespace-nowrap *:truncate">{loading ? null : children}</div>
  </div>
}

export function SectionHeading({ icon: Icon, title, children }: { icon: LucideIcon; title: string; children?: ReactNode }) {
  return <div className="flex items-center justify-between gap-3 max-sm:flex-col max-sm:items-stretch">
    <h2 className="flex items-center gap-2 text-sm font-semibold"><Icon className="text-primary size-4" aria-hidden="true" />{title}</h2>
    {children}
  </div>
}
