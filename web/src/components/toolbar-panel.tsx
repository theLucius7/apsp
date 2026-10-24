import { useState } from 'react'
import type { ReactNode } from 'react'
import type { LucideIcon } from 'lucide-react'
import { useI18n } from '../i18n'
import { useMediaQuery } from '@/lib/use-media-query'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover'

/**
 * A toolbar button for secondary options, such as extra filters or view
 * settings. On wider screens the options open in a popover beside the button;
 * on phones they open in a drawer along the bottom edge. The number on the
 * button says how many of the options are in use.
 */
export function ToolbarPanel({ icon: Icon, label, title, count = 0, children }: { icon: LucideIcon; label: string; title: string; count?: number; children: ReactNode }) {
  const { t } = useI18n()
  const wide = useMediaQuery('(min-width: 640px)')
  const [open, setOpen] = useState(false)
  const button = <Button variant="outline" data-slot="toolbar-panel-trigger" aria-label={t(label)} aria-haspopup="dialog" aria-expanded={open} onClick={wide ? undefined : () => setOpen(true)}>
    <Icon aria-hidden="true" /><span>{t(label)}</span>
    {count > 0 && <span data-slot="toolbar-panel-count" aria-hidden="true" className="bg-primary text-primary-foreground grid h-4.5 min-w-4.5 place-items-center rounded-full px-1 text-[11px] font-medium tabular-nums">{count}</span>}
  </Button>
  if (wide) return <Popover open={open} onOpenChange={setOpen}>
    <PopoverTrigger asChild>{button}</PopoverTrigger>
    <PopoverContent align="end" aria-label={t(title)} className="w-80">{children}</PopoverContent>
  </Popover>
  return <>{button}<Dialog open={open} onOpenChange={setOpen}>
    <DialogContent placement="bottom" aria-describedby={undefined} closeLabel={t('关闭')}>
      <DialogHeader><DialogTitle>{t(title)}</DialogTitle></DialogHeader>
      <div className="grid gap-4">{children}</div>
      <Button onClick={() => setOpen(false)}>{t('完成')}</Button>
    </DialogContent>
  </Dialog></>
}
