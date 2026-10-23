import type { CSSProperties, ReactNode } from 'react'
import { DndContext, KeyboardSensor, PointerSensor, closestCenter, useSensor, useSensors } from '@dnd-kit/core'
import type { Announcements, DragEndEvent, UniqueIdentifier } from '@dnd-kit/core'
import { SortableContext, sortableKeyboardCoordinates, useSortable, verticalListSortingStrategy } from '@dnd-kit/sortable'
import { restrictToParentElement, restrictToVerticalAxis } from '@dnd-kit/modifiers'
import { CSS } from '@dnd-kit/utilities'
import { GripVerticalIcon } from 'lucide-react'
import { cssTransition } from '@/lib/motion'
import { Button } from '@/components/ui/button'

export type SortableItemProps = {
  ref: (element: HTMLElement | null) => void
  style: CSSProperties
  'data-dragging'?: true
}

/** Lift and outline the row being dragged; rows still read as rows at rest. */
export const sortableItemClass = 'data-dragging:bg-card data-dragging:ring-primary/30 data-dragging:relative data-dragging:z-10 data-dragging:rounded-lg data-dragging:shadow-lg data-dragging:ring-2'

type Props<T> = {
  items: readonly T[]
  /** Stable identity; defaults to the position. */
  getId?: (item: T, index: number) => string
  getLabel: (item: T, index: number) => string
  onMove: (from: number, to: number) => void
  disabled?: boolean
  children: (item: T, index: number, item_props: SortableItemProps, handle: ReactNode) => ReactNode
}

/**
 * Vertical drag-and-drop ordering. Each row gets a grip handle that works with a
 * pointer, touch or the keyboard (Space to lift, arrow keys to move, Space to drop,
 * Escape to cancel); screen readers hear the position as it changes.
 */
export function SortableList<T>({ items, getId = (_, index) => `position-${index}`, getLabel, onMove, disabled = false, children }: Props<T>) {
  const sensors = useSensors(
    useSensor(PointerSensor, { activationConstraint: { distance: 4 } }),
    useSensor(KeyboardSensor, { coordinateGetter: sortableKeyboardCoordinates }),
  )
  const ids = items.map((item, index) => getId(item, index))
  const label = (id: UniqueIdentifier) => { const index = ids.indexOf(String(id)); return index < 0 ? '此项' : getLabel(items[index], index) }
  const position = (id: UniqueIdentifier | undefined) => id === undefined ? 0 : ids.indexOf(String(id)) + 1
  const announcements: Announcements = {
    onDragStart: ({ active }) => `已拿起${label(active.id)}，当前位于第 ${position(active.id)} 位。`,
    onDragOver: ({ active, over }) => over ? `${label(active.id)}移动到第 ${position(over.id)} 位。` : `${label(active.id)}不在可放置的位置。`,
    onDragEnd: ({ active, over }) => over ? `${label(active.id)}已放在第 ${position(over.id)} 位。` : `${label(active.id)}已放下，顺序未变。`,
    onDragCancel: ({ active }) => `已取消移动${label(active.id)}。`,
  }
  const end = ({ active, over }: DragEndEvent) => {
    if (!over || active.id === over.id) return
    const from = ids.indexOf(String(active.id)), to = ids.indexOf(String(over.id))
    if (from >= 0 && to >= 0) onMove(from, to)
  }
  return <DndContext sensors={sensors} collisionDetection={closestCenter} modifiers={[restrictToVerticalAxis, restrictToParentElement]} onDragEnd={end}
    accessibility={{ announcements, screenReaderInstructions: { draggable: '按空格键拿起此项，用上下方向键移动位置，再按空格键放下；按 Esc 取消。' } }}>
    <SortableContext items={ids} strategy={verticalListSortingStrategy} disabled={disabled}>
      {items.map((item, index) => <SortableRow key={ids[index]} id={ids[index]} label={getLabel(item, index)} disabled={disabled}>
        {(props, handle) => children(item, index, props, handle)}
      </SortableRow>)}
    </SortableContext>
  </DndContext>
}

function SortableRow({ id, label, disabled, children }: { id: string; label: string; disabled: boolean; children: (props: SortableItemProps, handle: ReactNode) => ReactNode }) {
  const { attributes, listeners, setNodeRef, setActivatorNodeRef, transform, transition, isDragging } = useSortable({ id, disabled, transition: cssTransition })
  const handle = <Button ref={setActivatorNodeRef} type="button" variant="ghost" size="icon-sm" disabled={disabled}
    className="text-muted-foreground cursor-grab touch-none active:cursor-grabbing" {...attributes} {...listeners} aria-label={`拖动排序：${label}`}>
    <GripVerticalIcon aria-hidden="true" />
  </Button>
  return children({ ref: setNodeRef, style: { transform: CSS.Translate.toString(transform), transition }, ...(isDragging ? { 'data-dragging': true } : {}) }, handle)
}
