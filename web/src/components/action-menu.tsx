import { Fragment, useRef } from 'react'
import { EllipsisIcon } from 'lucide-react'
import { useI18n } from '../i18n'
import { Button } from '@/components/ui/button'
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuSeparator, DropdownMenuTrigger } from '@/components/ui/dropdown-menu'

export type MenuAction = { label: string; onSelect: () => void; disabled?: boolean; destructive?: boolean }

/**
 * Less frequent row actions behind one “更多操作” button, placed after the
 * frequent actions that stay visible. Groups are divided by a rule and empty
 * groups are dropped. Choosing an item first returns focus to the button, so a
 * dialog opened by the item hands focus back to it when it closes.
 */
export function ActionMenu({ groups, label = '更多操作' }: { groups: (MenuAction | false | null | undefined)[][]; label?: string }) {
  const { t } = useI18n()
  const trigger = useRef<HTMLButtonElement>(null)
  const visible = groups.map(group => group.filter((action): action is MenuAction => Boolean(action))).filter(group => group.length)
  if (!visible.length) return null
  return <DropdownMenu modal={false}>
    <DropdownMenuTrigger asChild>
      <Button ref={trigger} data-slot="action-menu-trigger" variant="ghost" size="icon-sm" aria-label={t(label)} title={t(label)}><EllipsisIcon aria-hidden="true" /></Button>
    </DropdownMenuTrigger>
    <DropdownMenuContent align="end">
      {visible.map((group, index) => <Fragment key={index}>
        {index > 0 && <DropdownMenuSeparator />}
        {group.map(action => <DropdownMenuItem key={action.label} disabled={action.disabled} variant={action.destructive ? 'destructive' : 'default'} onSelect={() => { trigger.current?.focus(); action.onSelect() }}>{t(action.label)}</DropdownMenuItem>)}
      </Fragment>)}
    </DropdownMenuContent>
  </DropdownMenu>
}
