import { useEffect } from 'react'

// One place decides whether the console may leave the current page while a form
// holds unsaved input: in-app links, the command palette, code that navigates and
// the browser's back and forward buttons all pass through it.

export const leaveMessage = '表单有未保存内容。确定离开此页面吗？'

const blockers = new Set<symbol>()
// A hash the user already agreed to open, so its hashchange does not ask twice.
let approved: string | null = null

/** The address as `location.hash` reports it, so comparisons see the same encoding. */
function normalized(path: string) {
  const link = document.createElement('a')
  link.href = path.startsWith('#') ? path : `#${path}`
  return link.hash
}

export function hasUnsavedChanges() {
  return blockers.size > 0
}

/** Asks once when unsaved input would be lost; true when leaving is fine. */
export function confirmLeave() {
  return blockers.size === 0 || window.confirm(leaveMessage)
}

/** Opens another console address, asking first when a form has unsaved input. */
export function navigate(path: string) {
  const hash = normalized(path)
  if (hash === window.location.hash) return true
  if (!confirmLeave()) return false
  approved = hash
  window.location.hash = hash
  return true
}

/**
 * Called by the router for every hash change. Changes made through navigate() or
 * an approved link were already confirmed; anything else (back and forward, code
 * that set the hash directly) asks now. When the user stays, the previous address
 * is restored without another hashchange.
 */
export function acceptHashChange(previous: string) {
  const hash = window.location.hash
  const confirmed = approved === hash
  approved = null
  if (confirmed || hash === previous || confirmLeave()) return true
  window.history.pushState(window.history.state, '', previous)
  return false
}

/** Asks before an in-app link replaces a page with unsaved input. Returns the cleanup. */
export function guardLinks() {
  const click = (event: MouseEvent) => {
    if (event.defaultPrevented || event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return
    const link = event.target instanceof Element ? event.target.closest<HTMLAnchorElement>('a[href^="#"]') : null
    if (!link || (link.target && link.target !== '_self') || link.hash === window.location.hash) return
    if (confirmLeave()) { approved = link.hash; return }
    event.preventDefault()
    event.stopImmediatePropagation()
  }
  document.addEventListener('click', click, true)
  return () => document.removeEventListener('click', click, true)
}

/** Keeps the page from being left, reloaded or closed silently while `active`. */
export function useLeaveGuard(active: boolean) {
  useEffect(() => {
    if (!active) return
    const token = Symbol('unsaved form')
    blockers.add(token)
    const unload = (event: BeforeUnloadEvent) => { event.preventDefault(); event.returnValue = '' }
    window.addEventListener('beforeunload', unload)
    return () => { blockers.delete(token); window.removeEventListener('beforeunload', unload) }
  }, [active])
}
