import type { ReactNode } from 'react'
import { useDefaultLayout } from 'react-resizable-panels'
import { useI18n } from '../i18n'
import { useMediaQuery } from '@/lib/use-media-query'
import { layoutStorage } from '@/lib/layout-storage'
import { ResizableHandle, ResizablePanel, ResizablePanelGroup } from '@/components/ui/resizable'

/**
 * A list beside its detail view. On wide screens the divider can be dragged (or
 * moved with the arrow keys) and the width is remembered; narrow screens stack
 * the two sections.
 */
export function ListDetailLayout({ id, list, detail, listSize = 280, minListSize = 220, maxListSize = 440, label = '调整列表宽度' }: {
  id: string
  list: ReactNode
  detail: ReactNode
  listSize?: number
  minListSize?: number
  maxListSize?: number
  label?: string
}) {
  const { t } = useI18n()
  const desktop = useMediaQuery('(min-width: 768px)')
  const layout = useDefaultLayout({ id, storage: layoutStorage, onlySaveAfterUserInteractions: true })
  if (!desktop) return <div data-slot="list-detail" className="grid min-w-0 gap-4">{list}{detail}</div>
  return <ResizablePanelGroup data-slot="list-detail" id={id} orientation="horizontal" defaultLayout={layout.defaultLayout} onLayoutChanged={layout.onLayoutChanged} className="mb-4 items-start">
    <ResizablePanel id="list" defaultSize={listSize} minSize={minListSize} maxSize={maxListSize}>
      <div className="min-w-0 p-1 pr-2">{list}</div>
    </ResizablePanel>
    <ResizableHandle withHandle aria-label={t(label)} className="self-stretch" />
    <ResizablePanel id="detail" minSize="45">
      <div className="min-w-0 p-1 pl-2">{detail}</div>
    </ResizablePanel>
  </ResizablePanelGroup>
}
