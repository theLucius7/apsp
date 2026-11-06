import type { ReactNode } from 'react'
import type { LucideIcon } from 'lucide-react'
import { CheckIcon } from 'lucide-react'
import { BrandMark } from '../components'
import { useI18n } from '../i18n'
import { cn } from '@/lib/utils'

// Every page a visitor sees before signing in (administrator, proxy user,
// invitation) shares one layout: the mark inside a slowly turning network, a
// title, one card and a quiet footer. Nothing here may describe what the
// panel does: before sign-in the only name shown is "apsp" (#219).

function AuthArt() {
  return <div className="relative mx-auto -mt-4 size-40" aria-hidden="true">
    <span className="absolute inset-5 rounded-full bg-[radial-gradient(closest-side,color-mix(in_oklch,var(--brand-from)_28%,transparent),transparent)]" />
    <div className="absolute inset-0 motion-safe:animate-[orbit-turn_60s_linear_infinite]">
      <svg viewBox="0 0 160 160" className="size-full" fill="none">
        <circle cx="80" cy="80" r="74" className="stroke-primary/25" strokeDasharray="2 7" strokeLinecap="round" />
        <circle cx="80" cy="80" r="54" className="stroke-info/30" strokeDasharray="1.5 6" strokeLinecap="round" />
        <path d="M80 6 Q128 26 134 80" className="stroke-primary/25" strokeWidth="1" />
        <circle cx="80" cy="6" r="3.2" className="fill-primary/80" /><circle cx="134" cy="80" r="2.4" className="fill-info/80" />
        <circle cx="42" cy="118" r="2.8" className="fill-chart-4/70" /><circle cx="28" cy="52" r="1.8" className="fill-primary/50" />
      </svg>
    </div>
    <div className="absolute inset-0 grid place-items-center"><BrandMark className="size-16 drop-shadow-[0_12px_28px_color-mix(in_oklch,var(--brand-from)_45%,transparent)]" /></div>
  </div>
}

export function AuthShell({ slot, title, description, corner, steps, footer, children }: {
  slot: string; title: string; description?: ReactNode; corner?: ReactNode; steps?: ReactNode; footer?: ReactNode; children: ReactNode
}) {
  const { t } = useI18n()
  return <main data-slot={slot} className="relative grid min-h-dvh place-items-center px-5 py-12 sm:px-8">
    {corner && <div className="absolute top-4 right-4">{corner}</div>}
    <div className="mx-auto w-full max-w-[26rem] min-w-0">
      <AuthArt />
      <p className="-mt-2 mb-7 text-center text-lg font-semibold tracking-[-0.02em]">{t('apsp')}</p>
      <h1 className="text-center text-[28px] leading-tight font-semibold tracking-[-0.02em]">{title}</h1>
      {description && <p className="text-muted-foreground mt-1.5 text-center text-sm leading-relaxed">{description}</p>}
      {steps}
      <div className="bg-card shadow-lifted mt-6 rounded-[20px] border p-6 sm:p-7">{children}</div>
      {footer && <div className="text-muted-foreground mt-5 text-center text-xs leading-relaxed">{footer}</div>}
    </div>
  </main>
}

/** A labelled input with an icon at its start; the label names the input alone. `action` sits at the label's end. */
export function AuthField({ id, label, icon: Icon, action, children }: { id: string; label: string; icon: LucideIcon; action?: ReactNode; children: ReactNode }) {
  return <div className="grid gap-1.5">
    <div className="flex min-h-5 items-center justify-between gap-3"><label htmlFor={id} className="text-[13px] font-medium">{label}</label>{action}</div>
    <div className="relative"><Icon className="text-muted-foreground pointer-events-none absolute top-1/2 left-3.5 z-10 size-4 -translate-y-1/2" aria-hidden="true" />{children}</div>
  </div>
}

export function AuthDivider({ label }: { label: string }) {
  return <div className="my-5 flex items-center gap-3 text-xs"><span className="bg-border h-px flex-1" aria-hidden="true" /><span className="text-muted-foreground">{label}</span><span className="bg-border h-px flex-1" aria-hidden="true" /></div>
}

/** Where a visitor is in a short flow: finished steps are ticked and joined to the lit current one. */
export function AuthSteps({ steps, current }: { steps: string[]; current: number }) {
  return <ol data-slot="auth-steps" aria-label="步骤" className="mx-auto mt-5 flex max-w-80 items-start text-xs">
    {steps.map((step, index) => <li key={step} aria-current={index === current ? 'step' : undefined} className="relative flex min-w-0 flex-1 flex-col items-center gap-1.5 text-center">
      {index > 0 && <span className={cn('absolute top-3 right-1/2 h-px w-full', index <= current ? 'bg-primary/60' : 'bg-border')} aria-hidden="true" />}
      <span className={cn('relative grid size-6 place-items-center rounded-full border text-[11px] font-semibold tabular-nums',
        index < current ? 'border-primary bg-primary text-primary-foreground' : index === current ? 'border-primary bg-card text-primary shadow-focus' : 'bg-card text-muted-foreground')}>
        {index < current ? <CheckIcon className="size-3.5" aria-hidden="true" /> : index + 1}
      </span>
      <span className={index === current ? 'font-medium' : 'text-muted-foreground'}>{step}{index < current && <span className="sr-only">（已完成）</span>}</span>
    </li>)}
  </ol>
}
