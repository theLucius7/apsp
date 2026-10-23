import { useEffect, useRef, useState } from 'react'
import type { FormEvent, ReactNode } from 'react'
import { m } from 'motion/react'
import { BoxIcon, CheckIcon, CircleAlertIcon, CompassIcon, CopyIcon, InfoIcon, RefreshCwIcon } from 'lucide-react'
import type { LucideIcon } from 'lucide-react'
import { errorMessage } from './api'
import { resourceRefreshingMessage } from './hooks'
import { useI18n } from './i18n'
import { cn } from '@/lib/utils'
import { revealMotion } from '@/lib/motion'
import { Alert, AlertDescription } from '@/components/ui/alert'
import { AlertDialog, AlertDialogCancel, AlertDialogContent, AlertDialogFooter, AlertDialogHeader, AlertDialogTitle } from '@/components/ui/alert-dialog'
import { Badge as UiBadge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { Empty as UiEmpty, EmptyContent, EmptyDescription, EmptyHeader, EmptyMedia, EmptyTitle } from '@/components/ui/empty'
import { Progress } from '@/components/ui/progress'
import { Spinner } from '@/components/ui/spinner'

export type Tone = 'good' | 'bad' | 'warm' | 'neutral'
const toneVariant = { good: 'success', bad: 'destructive', warm: 'warning', neutral: 'secondary' } as const

export function Brand({ className }: { className?: string }) {
  const { t } = useI18n()
  return <div data-slot="brand" className={cn('flex min-w-0 items-center gap-2.5', className)}>
    <span className="bg-primary text-primary-foreground grid size-8 shrink-0 place-items-center rounded-lg"><CompassIcon className="size-4.5" aria-hidden="true" /></span>
    <span className="grid min-w-0 leading-tight">
      <strong className="truncate text-base font-semibold tracking-wide">{t('apsp')}</strong>
      <small className="text-muted-foreground truncate text-xs">{t('服务器与节点')}</small>
    </span>
  </div>
}

export function Badge({ children, tone = 'neutral', className }: { children: ReactNode; tone?: Tone; className?: string }) {
  return <UiBadge data-tone={tone} variant={toneVariant[tone]} className={className}>
    <span className="size-1.5 shrink-0 rounded-full bg-current" aria-hidden="true" />{children}
  </UiBadge>
}

export function PageHeader({ eyebrow, title, description, children }: { eyebrow: string; title: string; description: string; children?: ReactNode }) {
  const { t } = useI18n()
  return <header data-slot="page-header" className="mb-6 flex flex-col gap-4 md:flex-row md:items-end md:justify-between">
    <div className="min-w-0 space-y-1">
      <div className="text-primary text-xs font-medium tracking-wide">{t(eyebrow)}</div>
      <h1 className="text-xl font-semibold tracking-tight text-balance">{t(title)}</h1>
      <p className="text-muted-foreground max-w-3xl text-sm text-pretty">{t(description)}</p>
    </div>
    {children && <div data-slot="page-actions" className="flex shrink-0 flex-wrap items-center gap-2">{children}</div>}
  </header>
}

export function ErrorNotice({ message, retry }: { message?: string; retry?: () => void }) {
  const { t } = useI18n()
  const transient = message === resourceRefreshingMessage
  const [showRefresh, setShowRefresh] = useState(false)
  useEffect(() => {
    setShowRefresh(false)
    if (!transient) return
    const timer = window.setTimeout(() => setShowRefresh(true), 250)
    return () => window.clearTimeout(timer)
  }, [message, transient])
  const visible = Boolean(message) && (!transient || showRefresh)
  // A resolved error leaves at once so it never lingers beside recovered data.
  return visible && <m.div key="error" initial={revealMotion.initial} animate={revealMotion.animate} transition={revealMotion.transition} className="overflow-hidden">
    <Alert variant="destructive" className="mb-4 flex items-start justify-between gap-3">
      <span className="flex min-w-0 items-start gap-2"><CircleAlertIcon className="mt-0.5 size-4 shrink-0" aria-hidden="true" /><span className="min-w-0 break-words">{message}</span></span>
      {retry && <Button variant="outline" size="xs" type="button" onClick={retry}>{t('重试')}</Button>}
    </Alert>
  </m.div>
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

export function Loading() {
  const { t } = useI18n()
  return <div data-slot="loading" role="status" className="text-muted-foreground flex min-h-40 items-center justify-center gap-2 text-sm">
    <Spinner role="presentation" aria-hidden="true" aria-label={undefined} />{t('正在加载…')}
  </div>
}

export function Refresh({ onClick }: { onClick: () => void }) {
  const { t } = useI18n()
  return <Button variant="outline" onClick={onClick}><RefreshCwIcon aria-hidden="true" /><span>{t('刷新')}</span></Button>
}

export function Stat({ label, value, note, icon: Icon }: { label: string; value: ReactNode; note?: ReactNode; icon: LucideIcon }) {
  const { t } = useI18n()
  return <div data-slot="stat" className="bg-card text-card-foreground ring-foreground/10 min-w-0 rounded-xl p-4 ring-1">
    <div className="text-muted-foreground flex items-center justify-between gap-2 text-xs">{t(label)}<Icon className="size-4 shrink-0 max-sm:hidden" aria-hidden="true" /></div>
    <strong className="mt-2 block text-2xl font-semibold tracking-tight tabular-nums">{value}</strong>
    {note && <small className="text-muted-foreground mt-1 block text-xs">{note}</small>}
  </div>
}

export function Meter({ value, className }: { value?: number; className?: string }) {
  return <Progress value={Math.min(100, Math.max(0, value ?? 0))} className={cn('mt-1.5 h-1 max-w-28', className)} />
}

export function Modal({ title, children, onClose, busy = false, wide = false, className = '', slot }: { title: string; children: ReactNode; onClose: () => void; busy?: boolean; wide?: boolean; className?: string; slot?: string }) {
  const { t } = useI18n()
  const guard = (event: Event) => { if (busy) event.preventDefault() }
  // Dialogs render at the document root so hidden page sections, such as inactive
  // tabs, never hide a dialog that one of their components opened.
  return <Dialog open onOpenChange={open => { if (!open && !busy) onClose() }}>
    <DialogContent
      data-dialog={slot}
      aria-describedby={undefined}
      closeLabel={t('关闭对话框')}
      onEscapeKeyDown={guard}
      onInteractOutside={guard}
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

export function Confirm({ title, children, busy, disabled = false, confirmDisabled = false, confirmLabel = '确认删除', busyLabel = '正在删除…', error, retry, onClose, onConfirm }: { title: string; children: ReactNode; busy: boolean; disabled?: boolean; confirmDisabled?: boolean; confirmLabel?: string; busyLabel?: string; error?: string; retry?: () => void; onClose: () => void; onConfirm: () => void }) {
  const { t } = useI18n()
  return <AlertDialog open onOpenChange={open => { if (!open && !busy) onClose() }}>
    <AlertDialogContent aria-describedby={undefined} onEscapeKeyDown={event => { if (busy) event.preventDefault() }} className="sm:max-w-md">
      <AlertDialogHeader className="place-items-start text-left">
        <AlertDialogTitle>{t(title)}</AlertDialogTitle>
      </AlertDialogHeader>
      <div className="grid gap-3">
        <ErrorNotice message={error} retry={retry} />
        <p className="text-muted-foreground text-sm leading-relaxed break-words">{children}</p>
      </div>
      <AlertDialogFooter>
        <AlertDialogCancel disabled={busy}>{t('取消')}</AlertDialogCancel>
        <Button variant="destructive" disabled={busy || disabled || confirmDisabled} onClick={() => { if (!busy && !disabled && !confirmDisabled) onConfirm() }}>
          {busy && <Spinner role="presentation" aria-hidden="true" aria-label={undefined} />}{busy ? t(busyLabel) : t(confirmLabel)}
        </Button>
      </AlertDialogFooter>
    </AlertDialogContent>
  </AlertDialog>
}

export function Field({ label, hint, children, className }: { label: string; hint?: string; children: ReactNode; className?: string }) {
  const { t } = useI18n()
  return <label data-slot="field" className={cn('mb-4 grid min-w-0 gap-1.5 last:mb-0', className)}>
    <span className="text-sm leading-snug font-medium">{t(label)}</span>
    {children}
    {hint && <small className="text-muted-foreground text-xs leading-relaxed">{t(hint)}</small>}
  </label>
}

export function CopyButton({ text, label = '复制' }: { text: string; label?: string }) {
  const { t } = useI18n()
  const [copied, setCopied] = useState(false)
  const [error, setError] = useState('')
  const alive = useRef(true)
  useEffect(() => { alive.current = true; return () => { alive.current = false } }, [])
  useEffect(() => { if (!copied) return; const timer = setTimeout(() => setCopied(false), 2000); return () => clearTimeout(timer) }, [copied])
  const copy = async () => {
    try {
      if (navigator.clipboard?.writeText) await navigator.clipboard.writeText(text)
      else {
        const textarea = document.createElement('textarea'); textarea.value = text; textarea.style.position = 'fixed'; textarea.style.opacity = '0'; document.body.append(textarea); textarea.select()
        const success = document.execCommand('copy'); textarea.remove(); if (!success) throw new Error(t('浏览器禁止自动复制，请选中文字手动复制。'))
      }
      if (alive.current) { setCopied(true); setError('') }
    } catch (error) { if (alive.current) setError(errorMessage(error)) }
  }
  return <span data-slot="copy-control" className="inline-flex shrink-0 flex-col items-end gap-1">
    <Button variant="outline" size="sm" onClick={() => void copy()} type="button">{copied ? <CheckIcon aria-hidden="true" /> : <CopyIcon aria-hidden="true" />}{copied ? t('已复制') : t(label)}</Button>
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
