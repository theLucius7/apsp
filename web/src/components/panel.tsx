import type * as React from 'react'
import { cva, type VariantProps } from 'class-variance-authority'
import { cn } from '@/lib/utils'

// Page sections share the console card surface: a subtly grounded header row with its
// actions, then a table or padded content.
export function Panel({ className, ...props }: React.ComponentProps<'section'>) {
  return <section data-slot="panel" className={cn('bg-card text-card-foreground border-border/85 dark:border-border shadow-soft mb-4 min-w-0 overflow-hidden rounded-xl border', className)} {...props} />
}

export function PanelHeader({ className, ...props }: React.ComponentProps<'div'>) {
  return <div data-slot="panel-header" className={cn('border-border/75 bg-muted/20 dark:bg-muted/15 flex min-h-12 flex-wrap items-center justify-between gap-x-3 gap-y-2 border-b px-4 py-2', className)} {...props} />
}

export function PanelTitle({ className, ...props }: React.ComponentProps<'h2'>) {
  return <h2 data-slot="panel-title" className={cn('text-foreground flex min-w-0 items-center gap-2 text-sm leading-5 font-semibold tracking-[-0.01em]', className)} {...props} />
}

const noticeVariants = cva('mb-4 flex items-start justify-between gap-3 rounded-xl border px-3.5 py-2.5 text-[13px] leading-relaxed', {
  variants: {
    tone: {
      info: 'border-info/25 bg-info/6 text-foreground dark:border-info/30 dark:bg-info/10',
      error: 'border-destructive/30 bg-destructive/6 text-destructive dark:border-destructive/35 dark:bg-destructive/12 break-words',
      warning: 'border-warning/30 bg-warning/7 text-foreground dark:border-warning/35 dark:bg-warning/10',
      success: 'border-success/28 bg-success/6 text-foreground dark:border-success/35 dark:bg-success/10',
      quiet: 'border-border/75 bg-muted/45 text-foreground justify-start',
    },
  },
  defaultVariants: { tone: 'info' },
})

/** Inline status text inside a page or dialog; errors use ErrorNotice instead. */
export function Notice({ className, tone, ...props }: React.ComponentProps<'div'> & VariantProps<typeof noticeVariants>) {
  return <div data-slot="notice" data-tone={tone ?? 'info'} className={cn(noticeVariants({ tone }), className)} {...props} />
}
