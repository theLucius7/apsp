import type * as React from 'react'
import { cn } from '@/lib/utils'

// Section switchers that keep link or toggle-button semantics (aria-current /
// aria-pressed) while sharing the shadcn tabs appearance.
const listClass = 'bg-muted text-muted-foreground inline-flex max-w-full flex-wrap items-center gap-1 rounded-lg p-[3px]'
const itemClass = 'hover:text-foreground focus-visible:ring-ring/50 aria-[current=page]:bg-background aria-[current=page]:text-foreground aria-pressed:bg-background aria-pressed:text-foreground dark:aria-[current=page]:bg-input/40 dark:aria-pressed:bg-input/40 inline-flex h-7 min-w-0 items-center justify-center gap-1.5 rounded-md px-2.5 text-sm font-medium whitespace-nowrap transition-colors outline-none focus-visible:ring-3 aria-[current=page]:shadow-sm aria-pressed:shadow-sm disabled:pointer-events-none disabled:opacity-50 pointer-coarse:h-9 [&_svg:not([class*=size-])]:size-4'

export function SegmentedNav({ className, ...props }: React.ComponentProps<'nav'>) {
  return <nav data-slot="segmented" className={cn(listClass, className)} {...props} />
}

export function SegmentedGroup({ className, ...props }: React.ComponentProps<'div'>) {
  return <div role="group" data-slot="segmented" className={cn(listClass, className)} {...props} />
}

export function SegmentedButton({ className, type = 'button', ...props }: React.ComponentProps<'button'>) {
  return <button type={type} data-slot="segmented-item" className={cn(itemClass, className)} {...props} />
}

export function SegmentedLink({ className, ...props }: React.ComponentProps<'a'>) {
  return <a data-slot="segmented-item" className={cn(itemClass, className)} {...props} />
}
