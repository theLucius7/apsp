import { useState } from 'react'
import { LinkIcon } from 'lucide-react'
import { useI18n } from '../i18n'
import { cn } from '@/lib/utils'

export type Chip = { label: string; kind?: 'node' | 'chain' | 'external' }

/**
 * A list of names as compact chips: the first few show, the rest fold behind
 * “+N”. Screen readers get the whole list as one sentence instead of the chips.
 */
export function ChipList({ items, empty, limit = 8 }: { items: Chip[]; empty: string; limit?: number }) {
  const { t } = useI18n()
  const [open, setOpen] = useState(false)
  if (!items.length) return <span className="text-muted-foreground">{t(empty)}</span>
  const shown = open ? items : items.slice(0, limit), hidden = items.length - shown.length
  return <div className="min-w-0">
    <span className="sr-only">{items.map(item => item.kind === 'external' ? `${item.label}（外部）` : item.label).join('、')}</span>
    <div className="flex flex-wrap gap-1">
      <ul className="contents" aria-hidden="true">
        {shown.map((item, index) => <li key={`${item.label}-${index}`} title={item.label}
          className={cn('bg-card/80 inline-flex max-w-[15rem] items-center gap-1 rounded-md border px-1.5 py-0.5 text-xs leading-5',
            item.kind === 'external' && 'border-info/25 bg-info/8 text-info', item.kind === 'chain' && 'border-primary/25 bg-accent/60 text-accent-foreground')}>
          {item.kind === 'chain' && <LinkIcon className="size-3 shrink-0" />}
          <span className="truncate">{item.label}</span>
          {item.kind === 'external' && <span className="shrink-0 opacity-75">{t('外部')}</span>}
        </li>)}
      </ul>
      {items.length > limit && <button type="button" aria-expanded={open} onClick={() => setOpen(value => !value)}
        className="text-muted-foreground hover:text-foreground hover:bg-muted focus-visible:shadow-focus inline-flex h-[22px] items-center rounded-md border border-dashed px-1.5 text-xs outline-none transition-colors">
        {open ? t('收起') : `+${hidden}`}</button>}
    </div>
  </div>
}
