import type * as React from 'react'
import { cn } from '@/lib/utils'

// Section switchers that keep link or toggle-button semantics (aria-current /
// aria-pressed). SegmentedNav moves between sections of a page and reads as tabs on
// a rule; SegmentedGroup chooses how something is shown and reads as a pill toggle.
const navClass = 'text-muted-foreground flex max-w-full items-center gap-1 overflow-x-auto border-b no-scrollbar'
const groupClass = 'bg-muted/80 text-muted-foreground inline-flex max-w-full flex-wrap items-center gap-0.5 rounded-[11px] border border-border/60 p-[3px]'
const itemClass = 'hover:text-foreground inline-flex min-w-0 items-center justify-center gap-1.5 whitespace-nowrap outline-none focus-visible:shadow-focus pointer-coarse:h-10 text-[13px] font-medium transition-[color,background-color,box-shadow,border-color] duration-150 disabled:pointer-events-none disabled:opacity-50 [&_svg]:size-3.5 '
  // Pill toggles: the chosen one is a lifted card; the others brighten on hover.
  + 'in-data-[slot=segmented]:h-8 in-data-[slot=segmented]:rounded-[8px] in-data-[slot=segmented]:px-3 in-data-[slot=segmented]:hover:bg-card/60 in-data-[slot=segmented]:aria-pressed:bg-card in-data-[slot=segmented]:aria-pressed:text-foreground in-data-[slot=segmented]:aria-pressed:shadow-soft in-data-[slot=segmented]:aria-[current=page]:bg-card in-data-[slot=segmented]:aria-[current=page]:text-foreground in-data-[slot=segmented]:aria-[current=page]:shadow-soft dark:in-data-[slot=segmented]:aria-pressed:bg-input/40 dark:in-data-[slot=segmented]:aria-[current=page]:bg-input/40 '
  // Tabs on a rule: the current one is underlined in the accent, the others hint on hover.
  + 'in-data-[slot=tabs]:-mb-px in-data-[slot=tabs]:h-10 in-data-[slot=tabs]:shrink-0 in-data-[slot=tabs]:border-b-2 in-data-[slot=tabs]:border-transparent in-data-[slot=tabs]:px-3 in-data-[slot=tabs]:hover:border-border in-data-[slot=tabs]:aria-pressed:border-primary in-data-[slot=tabs]:aria-pressed:text-foreground in-data-[slot=tabs]:aria-[current=page]:border-primary in-data-[slot=tabs]:aria-[current=page]:text-foreground'

export function SegmentedNav({ className, ...props }: React.ComponentProps<'nav'>) {
  return <nav data-slot="tabs" className={cn(navClass, className)} {...props} />
}

export function SegmentedGroup({ className, ...props }: React.ComponentProps<'div'>) {
  return <div role="group" data-slot="segmented" className={cn(groupClass, className)} {...props} />
}

export function SegmentedButton({ className, type = 'button', ...props }: React.ComponentProps<'button'>) {
  return <button type={type} data-slot="segmented-item" className={cn(itemClass, className)} {...props} />
}

export function SegmentedLink({ className, ...props }: React.ComponentProps<'a'>) {
  return <a data-slot="segmented-item" className={cn(itemClass, className)} {...props} />
}
