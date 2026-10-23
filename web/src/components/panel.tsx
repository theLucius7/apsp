import type * as React from 'react'
import { cva, type VariantProps } from 'class-variance-authority'
import { cn } from '@/lib/utils'

// Page sections share the shadcn card surface: a titled header row with its
// actions, then a table or padded content.
export function Panel({ className, ...props }: React.ComponentProps<'section'>) {
  return <section data-slot="panel" className={cn('bg-card text-card-foreground ring-foreground/10 mb-4 min-w-0 overflow-hidden rounded-xl ring-1', className)} {...props} />
}

export function PanelHeader({ className, ...props }: React.ComponentProps<'div'>) {
  return <div data-slot="panel-header" className={cn('flex min-h-14 flex-wrap items-center justify-between gap-3 border-b px-4 py-3', className)} {...props} />
}

export function PanelTitle({ className, ...props }: React.ComponentProps<'h2'>) {
  return <h2 data-slot="panel-title" className={cn('flex min-w-0 items-center gap-2 text-sm font-semibold', className)} {...props} />
}

const noticeVariants = cva('mb-4 flex items-start justify-between gap-3 rounded-lg border px-3 py-2.5 text-sm leading-relaxed', {
  variants: {
    tone: {
      info: 'bg-muted/60 text-foreground',
      error: 'border-destructive/25 bg-destructive/5 text-destructive break-words',
      success: 'border-success/25 bg-success/5 text-foreground',
      quiet: 'bg-muted/60 text-foreground justify-start',
    },
  },
  defaultVariants: { tone: 'info' },
})

/** Inline status text inside a page or dialog; errors use ErrorNotice instead. */
export function Notice({ className, tone, ...props }: React.ComponentProps<'div'> & VariantProps<typeof noticeVariants>) {
  return <div data-slot="notice" data-tone={tone ?? 'info'} className={cn(noticeVariants({ tone }), className)} {...props} />
}
