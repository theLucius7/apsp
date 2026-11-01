import type * as React from 'react'
import { cn } from '@/lib/utils'

// Section switchers that keep link or toggle-button semantics (aria-current /
// aria-pressed). SegmentedNav moves between sections of a page and reads as tabs on
// a rule; SegmentedGroup chooses how something is shown and reads as a pill toggle.
const navClass = 'text-muted-foreground flex max-w-full items-center gap-1 overflow-x-auto border-b no-scrollbar'
const groupClass = 'bg-muted text-muted-foreground inline-flex max-w-full flex-wrap items-center gap-0.5 rounded-lg p-0.5'
const itemClass = 'hover:text-foreground focus-visible:ring-ring/50 inline-flex min-w-0 items-center justify-center gap-1.5 whitespace-nowrap outline-none focus-visible:ring-3 pointer-coarse:h-10 text-[13px] font-medium transition-colors disabled:pointer-events-none disabled:opacity-50 [&_svg]:size-3.5 '
  + 'in-data-[slot=segmented]:h-7 in-data-[slot=segmented]:rounded-md in-data-[slot=segmented]:px-2.5 in-data-[slot=segmented]:aria-pressed:bg-background in-data-[slot=segmented]:aria-pressed:text-foreground in-data-[slot=segmented]:aria-[current=page]:bg-background in-data-[slot=segmented]:aria-[current=page]:text-foreground dark:in-data-[slot=segmented]:aria-pressed:bg-input/40 dark:in-data-[slot=segmented]:aria-[current=page]:bg-input/40 '
  + 'in-data-[slot=tabs]:-mb-px in-data-[slot=tabs]:h-9 in-data-[slot=tabs]:shrink-0 in-data-[slot=tabs]:border-b-2 in-data-[slot=tabs]:border-transparent in-data-[slot=tabs]:px-2.5 in-data-[slot=tabs]:aria-pressed:border-primary in-data-[slot=tabs]:aria-pressed:text-foreground in-data-[slot=tabs]:aria-[current=page]:border-primary in-data-[slot=tabs]:aria-[current=page]:text-foreground'

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
