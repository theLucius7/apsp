import { useCallback, useContext, useEffect, useId, useRef, useState } from 'react'
import type { FormEvent, ReactNode } from 'react'
import { AnimatePresence, usePresence } from 'motion/react'
import { BoxIcon, CheckIcon, CircleAlertIcon, CopyIcon, InfoIcon, RefreshCwIcon } from 'lucide-react'
import type { LucideIcon } from 'lucide-react'
import { errorMessage } from './api'
import { resourceRefreshingMessage } from './hooks'
import { SectionContext } from './app/section'
import { tierDescriptions, tierLabels } from './app/tiers'
import type { Tier } from './app/tiers'
import { useI18n } from './i18n'
import { SegmentedLink, SegmentedNav } from '@/components/segmented'
import { copyText } from '@/lib/clipboard'
import { cn } from '@/lib/utils'
import { Alert, AlertDescription } from '@/components/ui/alert'
import { AlertDialog, AlertDialogCancel, AlertDialogContent, AlertDialogFooter, AlertDialogHeader, AlertDialogTitle } from '@/components/ui/alert-dialog'
import { Badge as UiBadge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { Empty as UiEmpty, EmptyContent, EmptyDescription, EmptyHeader, EmptyMedia, EmptyTitle } from '@/components/ui/empty'
import { Progress } from '@/components/ui/progress'
import { Spinner } from '@/components/ui/spinner'
import { Skeleton } from '@/components/ui/skeleton'
export type Tone = 'good' | 'bad' | 'warm' | 'neutral'
const toneVariant = { good: 'success', bad: 'destructive', warm: 'warning', neutral: 'secondary' } as const
const refreshNoticeDelay = 1200

/**
 * The apsp mark: an “A” drawn as a route, entry and exit at its feet and the
 * relay at its peak, on a violet-to-cyan tile with a light from above.
 */
export function BrandMark({ className }: { className?: string }) {
  // Each mark owns its gradients: one defined inside a hidden copy would not paint.
  const id = `apsp-mark-${useId().replace(/:/g, '')}`
  return <svg viewBox="0 0 32 32" className={className} aria-hidden="true">
    <defs>
      <linearGradient id={`${id}-tile`} x1="3" y1="1" x2="29" y2="31" gradientUnits="userSpaceOnUse"><stop offset="0" stopColor="var(--brand-from)" /><stop offset="1" stopColor="var(--brand-to)" /></linearGradient>
      <linearGradient id={`${id}-light`} x1="0" y1="0" x2="0" y2="1"><stop offset="0" stopColor="white" stopOpacity=".32" /><stop offset=".55" stopColor="white" stopOpacity="0" /></linearGradient>
    </defs>
    <rect x="1" y="1" width="30" height="30" rx="9" fill={`url(#${id}-tile)`} />
    <rect x="1" y="1" width="30" height="30" rx="9" fill={`url(#${id}-light)`} />
    <rect x="1.5" y="1.5" width="29" height="29" rx="8.5" fill="none" stroke="white" strokeOpacity=".22" />
    <path d="M9.6 22.6 16 9.8l6.4 12.8" fill="none" stroke="white" strokeWidth="2.4" strokeLinecap="round" strokeLinejoin="round" />
    <path d="M12.5 18.3q3.5 2.5 7 0" fill="none" stroke="white" strokeOpacity=".78" strokeWidth="1.9" strokeLinecap="round" />
    <circle cx="16" cy="9.8" r="2.75" fill="white" /><circle cx="9.6" cy="22.6" r="2.4" fill="white" /><circle cx="22.4" cy="22.6" r="2.4" fill="white" />
  </svg>
}

export function Brand({ className, compact = false }: { className?: string; compact?: boolean }) {
  const { t } = useI18n()
  return <div data-slot="brand" className={cn('flex min-w-0 items-center gap-2.5', className)}>
    <BrandMark className={cn('shrink-0 drop-shadow-[0_4px_10px_color-mix(in_oklch,var(--brand-from)_38%,transparent)]', compact ? 'size-8' : 'size-10')} />
    <strong className={cn('min-w-0 truncate leading-tight font-semibold tracking-[-0.02em]', compact ? 'text-base' : 'text-xl')}>{t('apsp')}</strong>
  </div>}

export function Badge({ children, tone = 'neutral', className }: { children: ReactNode; tone?: Tone; className?: string }) {
  return <UiBadge data-tone={tone} variant={toneVariant[tone]} className={className}>
    <span className="size-1.5 shrink-0 rounded-full bg-current" aria-hidden="true" />{children}
  </UiBadge>
}

/**
 * Marks an experimental or frozen feature; the tooltip explains the tier. The
 * badge is decorative so headings, tabs and links keep their names; pages state
 * their tier in text with TierNotice.
 */
export function TierBadge({ tier, className }: { tier: Tier; className?: string }) {
  const { t } = useI18n()
  return <UiBadge aria-hidden="true" data-slot="tier-badge" data-tier={tier} variant={tier === 'frozen' ? 'secondary' : 'warning'} title={t(tierDescriptions[tier])} className={cn('px-1.5 text-[11px] font-medium', className)}>{t(tierLabels[tier])}</UiBadge>
}

/** States at the top of a page what its tier means for the administrator. */
export function TierNotice({ tier, children }: { tier: Tier; children?: ReactNode }) {
  const { t } = useI18n()
  return <Alert data-slot="tier-notice" data-tier={tier} variant={tier === 'frozen' ? 'default' : 'warning'} className="mb-4 flex items-start gap-2">
    <InfoIcon className="mt-0.5 size-4 shrink-0" aria-hidden="true" />
    <AlertDescription className="text-foreground">{t(tierDescriptions[tier])}{children && <> {children}</>}</AlertDescription>
  </Alert>
}

/**
 * The one bar at the top of a page: what this is, one line on what it is for, and
 * what can be done here. It stays in view while the page scrolls on wider screens,
 * so the actions are never out of reach. `leading` holds a way back.
 */
export function PageHeader({ title, description, leading, children }: { title: string; description?: string; leading?: ReactNode; children?: ReactNode }) {
  const { t } = useI18n()
  const tabs = useContext(SectionContext)
  return <><header data-slot="page-header" className="bg-background/80 supports-backdrop-filter:backdrop-blur-md border-border/60 -mx-4 -mt-4 mb-5 flex flex-wrap items-center gap-x-4 gap-y-2 border-b px-4 py-2.5 md:sticky md:top-0 md:z-20 md:-mx-6 md:min-h-16 md:px-6">
    {leading}
    <div className="min-w-0 flex-[1_1_16rem]">
      <h1 className="truncate text-lg leading-snug font-semibold tracking-tight md:text-xl">{t(title)}</h1>
      {description && <p title={t(description)} className="text-muted-foreground mt-0.5 text-xs leading-snug md:truncate">{t(description)}</p>}
    </div>
    {children && <div data-slot="page-actions" className="flex flex-wrap items-center gap-2">{children}</div>}
  </header>
  {tabs.length > 0 && <SegmentedNav aria-label={t('本节页面')} className="mb-4">
    {tabs.map(tab => <SegmentedLink key={tab.path} href={`#${tab.path}`} aria-current={tab.label === title ? 'page' : undefined}>{t(tab.label)}</SegmentedLink>)}
  </SegmentedNav>}
  </>
}

export function ErrorNotice({ message, retry }: { message?: string; retry?: () => void }) {
  const { t } = useI18n()
  const transient = message === resourceRefreshingMessage
  const [showRefresh, setShowRefresh] = useState(false)
  useEffect(() => {
    setShowRefresh(false)
    if (!transient) return
    const timer = window.setTimeout(() => setShowRefresh(true), refreshNoticeDelay)
    return () => window.clearTimeout(timer)
  }, [message, transient])
  // Waiting for a read is not a failure: it appears only when the wait is noticeable, and quietly.
  if (transient) return showRefresh && <p data-slot="refresh-pending" role="status" className="text-muted-foreground mb-3 flex items-center gap-2 text-xs"><Spinner role="presentation" aria-hidden="true" aria-label={undefined} />{t('正在同步最新信息，完成后可以提交；当前草稿已保留。')}</p>
  // A resolved error leaves at once so it never lingers beside recovered data.
  return Boolean(message) && <Alert variant="destructive" className="mb-4 flex items-start justify-between gap-3">
    <span className="flex min-w-0 items-start gap-2"><CircleAlertIcon className="mt-0.5 size-4 shrink-0" aria-hidden="true" /><span className="min-w-0 break-words">{message}</span></span>
    {retry && <Button variant="outline" size="xs" type="button" onClick={retry}>{t('重试')}</Button>}
  </Alert>
}

/** Explains why a click made during a background refresh did nothing. */
export function RefreshNotice() {
  const { t } = useI18n()
  return <Alert variant="info" role="status" className="mb-4 flex items-start gap-2">
    <InfoIcon className="mt-0.5 size-4 shrink-0" aria-hidden="true" />
    <AlertDescription className="text-foreground">{t('最新信息正在同步，本次操作未执行；请稍候再试。')}</AlertDescription>
  </Alert>
}

export function Empty({ icon: Icon = BoxIcon, title, description, children }: { icon?: LucideIcon; title: string; description: string; children?: ReactNode }) {
  const { t } = useI18n()
  return <UiEmpty className="py-10">
    <EmptyHeader>
      <EmptyMedia variant="icon"><Icon aria-hidden="true" /></EmptyMedia>
      <EmptyTitle><h3>{t(title)}</h3></EmptyTitle>
      <EmptyDescription>{t(description)}</EmptyDescription>
    </EmptyHeader>
    {children && <EmptyContent>{children}</EmptyContent>}
  </UiEmpty>
}

/**
 * Placeholder while the first snapshot loads. Lists, card grids and panels show
 * the shape of the coming content so the page does not jump when data arrives;
 * small areas keep the spinner.
 */
export function Loading({ variant = 'spinner', rows = 4 }: { variant?: 'spinner' | 'table' | 'cards' | 'panel'; rows?: number }) {
  const { t } = useI18n()
  if (variant === 'spinner') return <div data-slot="loading" role="status" className="text-muted-foreground flex min-h-40 items-center justify-center gap-2 text-sm">
    <Spinner role="presentation" aria-hidden="true" aria-label={undefined} />{t('正在加载…')}
  </div>
  const lines = Array.from({ length: rows }, (_, index) => index)
  return <div data-slot="loading" data-variant={variant} role="status" className="min-w-0">
    <span className="sr-only">{t('正在加载…')}</span>
    {variant === 'table' && <div aria-hidden="true" className="divide-y">{lines.map(index => <div key={index} className="flex items-center gap-4 px-4 py-3">
      <Skeleton className="size-8 shrink-0 rounded-lg" />
      <div className="grid min-w-0 flex-1 gap-1.5"><Skeleton className="h-3.5 w-2/5 max-w-56" /><Skeleton className="h-3 w-1/4 max-w-36" /></div>
      <Skeleton className="h-3.5 w-16 max-sm:hidden" /><Skeleton className="h-3.5 w-24 max-md:hidden" /><Skeleton className="h-7 w-14 rounded-lg" />
    </div>)}</div>}
    {variant === 'cards' && <div aria-hidden="true" className="mb-4 grid grid-cols-2 gap-2.5 lg:grid-cols-4">{lines.map(index => <div key={index} className="ring-foreground/10 grid gap-3 rounded-xl p-4 ring-1">
      <Skeleton className="h-3 w-1/2" /><Skeleton className="h-6 w-2/3" /><Skeleton className="h-3 w-3/4" />
    </div>)}</div>}
    {variant === 'panel' && <div aria-hidden="true" className="ring-foreground/10 mb-4 grid gap-3 rounded-xl p-4 ring-1">
      <Skeleton className="h-4 w-1/3" />{lines.map(index => <Skeleton key={index} className="h-3.5" style={{ width: `${90 - index * 12}%` }} />)}
    </div>}
  </div>
}

export function Refresh({ onClick }: { onClick: () => void }) {
  const { t } = useI18n()
  // Each click turns the icon once, so a refresh that changes nothing still shows it happened.
  const [turns, setTurns] = useState(0)
  return <Button variant="ghost" size="icon" title={t('刷新')} onClick={() => { setTurns(value => value + 1); onClick() }}>
    <RefreshCwIcon key={turns} aria-hidden="true" className={turns ? 'motion-safe:animate-[refresh-turn_600ms_cubic-bezier(0.4,0,0.2,1)]' : undefined} /><span className="sr-only">{t('刷新')}</span>
  </Button>
}

/** One headline number. `note` adds what the number alone does not say. */
export function Stat({ label, value, note }: { label: string; value: ReactNode; note?: ReactNode; icon?: LucideIcon }) {
  const { t } = useI18n()
  return <div data-slot="stat" className="bg-card text-card-foreground shadow-soft relative min-w-0 overflow-hidden rounded-xl border px-4 py-3.5">
    <span className="pointer-events-none absolute inset-0 bg-[radial-gradient(120%_90%_at_100%_0%,color-mix(in_oklch,var(--primary)_7%,transparent),transparent_60%)]" aria-hidden="true" />
    <div className="text-muted-foreground relative truncate text-xs tracking-wide">{t(label)}</div>
    <strong className="relative mt-1 block truncate text-[26px] leading-none font-semibold tracking-tight">{value}</strong>
    {note && <small title={typeof note === 'string' ? note : undefined} className="text-muted-foreground relative mt-1.5 block truncate text-xs">{note}</small>}
  </div>
}

/** Usage bar that turns warning and then destructive, matching the dashboard thresholds (75% / 90% by default). */
export function Meter({ value, label, warning = 75, danger = 90, className }: { value?: number; label?: string; warning?: number; danger?: number; className?: string }) {
  const level = Math.min(100, Math.max(0, value ?? 0))
  const tone = level >= danger ? 'bg-destructive' : level >= warning ? 'bg-warning' : undefined
  return <Progress value={level} aria-label={label} indicatorClassName={tone} className={cn('mt-1.5 h-1 max-w-28', className)} />
}

/**
 * Pages show dialogs conditionally (`{editing && <Modal …/>}`). Wrapped in
 * DialogPresence, a dialog the page removes first closes and plays its exit
 * animation, then leaves the document; unwrapped, it disappears at once.
 */
export function DialogPresence({ children }: { children: ReactNode }) {
  return <AnimatePresence>{children}</AnimatePresence>
}

// Page dialogs still playing their exit animation. A dialog that opens finishes
// them at once, so two dialogs never overlap.
const exiting = new Set<() => void>()

/** Whether the page still shows this dialog, and how it reports the end of its exit. */
function useDialogPresence() {
  const [present, safeToRemove] = usePresence()
  const finished = useRef(false)
  const done = useCallback(() => {
    if (finished.current) return
    finished.current = true
    safeToRemove?.()
  }, [safeToRemove])
  useEffect(() => {
    if (present) {
      finished.current = false
      for (const finish of [...exiting]) finish()
      return
    }
    exiting.add(done)
    return () => { exiting.delete(done) }
  }, [present, done])
  return { present, done }
}

export function Modal({ title, children, onClose, busy = false, wide = false, className = '', slot }: { title: string; children: ReactNode; onClose: () => void; busy?: boolean; wide?: boolean; className?: string; slot?: string }) {
  const { t } = useI18n()
  // False once the page has removed this dialog inside DialogPresence; it then
  // closes and reports when its exit animation is done.
  const { present, done } = useDialogPresence()
  const guard = (event: Event) => { if (busy) event.preventDefault() }
  // Dialogs render at the document root so hidden page sections, such as inactive
  // tabs, never hide a dialog that one of their components opened.
  return <Dialog open={present} onOpenChange={open => { if (!open && !busy && present) onClose() }}>
    <DialogContent
      data-dialog={slot}
      aria-describedby={undefined}
      closeLabel={t('关闭对话框')}
      onEscapeKeyDown={guard}
      onInteractOutside={guard}
      onExitComplete={done}
      className={cn(
        'flex max-h-[calc(100dvh-2rem)] flex-col gap-0 overflow-y-auto p-0 sm:max-w-md',
        wide && 'sm:max-w-2xl',
        '[&>footer]:bg-muted/50 [&>form>footer]:bg-muted/50 [&>footer]:flex [&>form>footer]:flex [&>footer]:flex-wrap [&>form>footer]:flex-wrap [&>footer]:items-center [&>form>footer]:items-center [&>footer]:justify-end [&>form>footer]:justify-end [&>footer]:gap-2 [&>form>footer]:gap-2 [&>footer]:border-t [&>form>footer]:border-t [&>footer]:px-4 [&>form>footer]:px-4 [&>footer]:py-3 [&>form>footer]:py-3',
        className,
      )}
    >
      <DialogHeader className="border-b px-4 py-3 pr-12">
        <DialogTitle className="text-base leading-snug font-semibold break-words">{t(title)}</DialogTitle>
      </DialogHeader>
      {children}
    </DialogContent>
  </Dialog>
}

export function FormDialog({ title, children, onClose, onSubmit, busy, disabled = false, submitDisabled = false, error, retry, submitLabel = '保存', wide = false, className = '', slot }: { title: string; children: ReactNode; onClose: () => void; onSubmit: (data: FormData) => void; busy: boolean; disabled?: boolean; submitDisabled?: boolean; error?: string; retry?: () => void; submitLabel?: string; wide?: boolean; className?: string; slot?: string }) {
  const { t } = useI18n()
  const submit = (event: FormEvent<HTMLFormElement>) => { event.preventDefault(); if (!busy && !disabled && !submitDisabled) onSubmit(new FormData(event.currentTarget)) }
  // A background refresh only holds the submit; the draft stays editable so typing
  // and draft rows are never lost to a briefly disabled fieldset.
  const locked = busy || (disabled && error !== resourceRefreshingMessage)
  return <Modal title={title} onClose={onClose} busy={busy} wide={wide} className={className} slot={slot}>
    <form onSubmit={submit} className="flex min-h-0 flex-col">
      <div data-slot="modal-body" className="p-4"><ErrorNotice message={error} retry={retry} /><fieldset disabled={locked}>{children}</fieldset></div>
      <footer>
        <Button variant="outline" type="button" onClick={onClose} disabled={busy}>{t('取消')}</Button>
        <Button disabled={busy || disabled || submitDisabled}>{busy && <Spinner role="presentation" aria-hidden="true" aria-label={undefined} />}{busy ? t('正在保存…') : t(submitLabel)}</Button>
      </footer>
    </form>
  </Modal>
}

export function Confirm({ title, children, busy, disabled = false, confirmDisabled = false, confirmLabel = '确认删除', busyLabel = '正在删除…', tone = 'destructive', error, retry, onClose, onConfirm }: { title: string; children: ReactNode; busy: boolean; disabled?: boolean; confirmDisabled?: boolean; confirmLabel?: string; busyLabel?: string; tone?: 'destructive' | 'default'; error?: string; retry?: () => void; onClose: () => void; onConfirm: () => void }) {
  const { t } = useI18n()
  const { present, done } = useDialogPresence()
  return <AlertDialog open={present} onOpenChange={open => { if (!open && !busy && present) onClose() }}>
    <AlertDialogContent aria-describedby={undefined} onEscapeKeyDown={event => { if (busy) event.preventDefault() }} onExitComplete={done} className="sm:max-w-md">
      <AlertDialogHeader className="place-items-start text-left">
        <AlertDialogTitle>{t(title)}</AlertDialogTitle>
      </AlertDialogHeader>
      <div className="grid gap-3">
        <ErrorNotice message={error} retry={retry} />
        <p className="text-muted-foreground text-sm leading-relaxed break-words">{children}</p>
      </div>
      <AlertDialogFooter>
        <AlertDialogCancel disabled={busy}>{t('取消')}</AlertDialogCancel>
        <Button variant={tone} disabled={busy || disabled || confirmDisabled} onClick={() => { if (!busy && !disabled && !confirmDisabled) onConfirm() }}>
          {busy && <Spinner role="presentation" aria-hidden="true" aria-label={undefined} />}{busy ? t(busyLabel) : t(confirmLabel)}
        </Button>
      </AlertDialogFooter>
    </AlertDialogContent>
  </AlertDialog>
}

/** A titled on/off setting with its explanation; the whole row toggles its control. */
export function SettingRow({ title, description, children, className }: { title: ReactNode; description?: ReactNode; children: ReactNode; className?: string }) {
  return <label data-slot="setting-row" className={cn('relative flex cursor-pointer items-center gap-4 border-b py-3.5 last:border-0 last:pb-0', className)}>
    <span className="min-w-0 flex-1"><strong className="block text-sm font-medium">{title}</strong>{description && <small className="text-muted-foreground mt-1 block text-xs leading-relaxed">{description}</small>}</span>
    {children}
  </label>
}

export function Field({ label, hint, children, className }: { label: string; hint?: string; children: ReactNode; className?: string }) {
  const { t } = useI18n()
  return <label data-slot="field" className={cn('mb-4 grid min-w-0 gap-1.5 last:mb-0', className)}>
    <span className="text-sm leading-snug font-medium">{t(label)}</span>
    {children}
    {hint && <small className="text-muted-foreground text-xs leading-relaxed">{t(hint)}</small>}
  </label>
}

export function CopyButton({ text, label = '复制', primary = false }: { text: string; label?: string; primary?: boolean }) {
  const { t } = useI18n()
  const [copied, setCopied] = useState(false)
  const [error, setError] = useState('')
  const alive = useRef(true)
  useEffect(() => { alive.current = true; return () => { alive.current = false } }, [])
  useEffect(() => { if (!copied) return; const timer = setTimeout(() => setCopied(false), 2000); return () => clearTimeout(timer) }, [copied])
  const copy = async () => {
    try {
      await copyText(text)
      if (alive.current) { setCopied(true); setError('') }
    } catch (error) { if (alive.current) setError(errorMessage(error)) }
  }
  return <span data-slot="copy-control" className="inline-flex shrink-0 flex-col items-end gap-1">
    <Button variant={primary ? 'default' : 'outline'} size="sm" onClick={() => void copy()} type="button">{copied ? <CheckIcon aria-hidden="true" /> : <CopyIcon aria-hidden="true" />}{copied ? t('已复制') : t(label)}</Button>
    {error && <small className="text-destructive max-w-36 text-xs" role="alert">{t('复制失败，请手动选择并复制。')}</small>}
  </span>
}

/** `singleLine` keeps commands on one scrollable line so the preview matches what is copied. */
export function CopyField({ text, label, singleLine = false }: { text: string; label: string; singleLine?: boolean }) {
  return <div data-slot="copy-field" className="bg-muted/50 my-3 flex items-start gap-3 rounded-lg border p-3 max-md:flex-col max-md:*:data-[slot=copy-control]:self-end">
    <code tabIndex={0} className={cn('text-foreground min-w-0 rounded-sm leading-relaxed focus-visible:ring-ring/50 focus-visible:ring-3 focus-visible:outline-none', singleLine ? 'scrollbar-thin max-w-full overflow-x-auto break-normal whitespace-pre max-md:self-stretch md:flex-1' : 'flex-1 break-all whitespace-pre-wrap')}>{text}</code>
    <CopyButton text={text} label={label} />
  </div>
}
