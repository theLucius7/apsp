import { useLayoutEffect, useRef, useState } from 'react'
import type { ReactNode } from 'react'
import { createPortal } from 'react-dom'
import { useDefaultLayout } from 'react-resizable-panels'
import { useI18n } from '../i18n'
import { useMediaQuery } from '@/lib/use-media-query'
import { layoutStorage } from '@/lib/layout-storage'
import { ResizableHandle, ResizablePanel, ResizablePanelGroup } from '@/components/ui/resizable'

/** An element that keeps its React content mounted while layouts hand it around. */
function useHost() {
  const [host] = useState(() => {
    const element = document.createElement('div')
    element.style.display = 'contents'
    return element
  })
  return host
}

/** The place in the current layout where a host is shown. */
function Slot({ host, className }: { host: HTMLElement; className: string }) {
  const slot = useRef<HTMLDivElement>(null)
  useLayoutEffect(() => {
    const element = slot.current
    if (!element) return
    element.appendChild(host)
    return () => { if (host.parentNode === element) element.removeChild(host) }
  }, [host])
  return <div ref={slot} className={className} />
}

/**
 * A list beside its detail view. On wide screens the divider can be dragged (or
 * moved with the arrow keys) and the width is remembered; narrow screens stack
 * the two sections. The list and detail render into stable hosts that the layout
 * only places, so crossing the breakpoint keeps their state, drafts and dialogs.
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
  const listHost = useHost(), detailHost = useHost()
  return <>
    {desktop ? <ResizablePanelGroup data-slot="list-detail" id={id} orientation="horizontal" defaultLayout={layout.defaultLayout} onLayoutChanged={layout.onLayoutChanged} className="mb-4 items-start">
      <ResizablePanel id="list" defaultSize={listSize} minSize={minListSize} maxSize={maxListSize}>
        <Slot host={listHost} className="min-w-0 p-1 pr-2" />
      </ResizablePanel>
      <ResizableHandle withHandle aria-label={t(label)} className="self-stretch" />
      <ResizablePanel id="detail" minSize="45">
        <Slot host={detailHost} className="min-w-0 p-1 pl-2" />
      </ResizablePanel>
    </ResizablePanelGroup> : <div data-slot="list-detail" className="grid min-w-0 gap-4">
      <Slot host={listHost} className="min-w-0" />
      <Slot host={detailHost} className="min-w-0" />
    </div>}
    {createPortal(list, listHost)}
    {createPortal(detail, detailHost)}
  </>
}
