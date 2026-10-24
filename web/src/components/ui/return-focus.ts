import * as React from "react"

/**
 * Most dialogs here open from page state or a keyboard shortcut rather than a
 * Radix trigger, so Radix has nothing to refocus when they close. Remember what
 * had focus when the dialog opened and return there; when that element has left
 * the page or can no longer take focus (a desktop table hidden behind the phone
 * layout), fall back to the main content so keyboard users keep their place.
 * A caller's own `onCloseAutoFocus` runs first and can take over by preventing
 * the default.
 */
export function useReturnFocus(open: boolean, onCloseAutoFocus?: (event: Event) => void) {
  const opener = React.useRef<HTMLElement | null>(null)
  React.useLayoutEffect(() => {
    if (!open) return
    const active = document.activeElement
    opener.current = active instanceof HTMLElement && active !== document.body ? active : null
  }, [open])
  return (event: Event) => {
    onCloseAutoFocus?.(event)
    if (event.defaultPrevented) return
    event.preventDefault()
    const fallback = document.querySelector<HTMLElement>("main")
    const target = opener.current?.isConnected ? opener.current : fallback
    target?.focus({ preventScroll: true })
    if (target !== fallback && document.activeElement !== target) fallback?.focus({ preventScroll: true })
  }
}
