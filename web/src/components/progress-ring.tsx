import { useId } from 'react'
import type { ReactNode } from 'react'
import { cn } from '@/lib/utils'
import type { Tone } from '@/components/board'

export type RingTone = Tone | 'brand'
const strokeTone: Record<Exclude<RingTone, 'brand'>, string> = { good: 'stroke-success', warning: 'stroke-warning', danger: 'stroke-destructive', info: 'stroke-info', muted: 'stroke-muted-foreground/60' }
const RADIUS = 22
const CIRCUMFERENCE = 2 * Math.PI * RADIUS

/**
 * A share (0–100) drawn as a ring that fills clockwise from the top, with the reading
 * in the middle; every ring in the console is this one, at different sizes. The
 * optional hollow dot marks where the calendar has got to: an arc past it is running
 * ahead of time. `brand` paints the arc in the logo's violet-to-cyan; `full` draws the
 * whole ring faintly, for an amount without a limit.
 */
export function ProgressRing({ value, marker = null, tone, label, full = false, className, children }: {
  value: number | null; marker?: number | null; tone: RingTone; label: string; full?: boolean; className?: string; children?: ReactNode
}) {
  const id = `ring-${useId().replace(/:/g, '')}`
  const share = full ? 1 : value === null ? 0 : Math.min(100, Math.max(0, value)) / 100
  const angle = marker === null ? null : Math.min(100, Math.max(0, marker)) / 100 * 2 * Math.PI - Math.PI / 2
  return <div data-slot="ring" role="img" aria-label={label} title={label} className={cn('relative size-14 shrink-0', className)}>
    <svg viewBox="0 0 56 56" className="size-full" aria-hidden="true">
      {/* The gradient runs corner to corner on screen, like the logo's tile. */}
      {tone === 'brand' && <defs><linearGradient id={id} x1="1" y1="0" x2="0" y2="1"><stop offset="0" stopColor="var(--brand-from)" /><stop offset="1" stopColor="var(--brand-to)" /></linearGradient></defs>}
      <circle cx="28" cy="28" r={RADIUS} fill="none" strokeWidth="5" className="stroke-foreground/10" />
      {share > 0 && <circle cx="28" cy="28" r={RADIUS} fill="none" strokeWidth="5" strokeLinecap="round" transform="rotate(-90 28 28)"
        stroke={tone === 'brand' ? `url(#${id})` : undefined} strokeDasharray={CIRCUMFERENCE} strokeDashoffset={CIRCUMFERENCE * (1 - share)}
        className={cn('transition-[stroke-dashoffset] duration-700 ease-out motion-reduce:transition-none', tone !== 'brand' && strokeTone[tone], full && 'opacity-40')} />}
      {angle !== null && <circle data-slot="cycle-marker" cx={28 + RADIUS * Math.cos(angle)} cy={28 + RADIUS * Math.sin(angle)} r="3.25" strokeWidth="1.5" className="fill-card stroke-foreground/70" />}
    </svg>
    <span className="absolute inset-0 grid place-content-center text-center leading-none tabular-nums">{children}</span>
  </div>
}
