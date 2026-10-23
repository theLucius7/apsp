import * as React from "react"

type OpenProps = {
  open?: boolean
  defaultOpen?: boolean
  onOpenChange?: (open: boolean) => void
}

/**
 * Lets a Radix root share its open state with its content, so the content can
 * stay mounted under Motion's AnimatePresence and play its exit animation.
 */
export function createPresence() {
  const OpenContext = React.createContext(false)

  function useRootOpen({ open: controlled, defaultOpen = false, onOpenChange }: OpenProps) {
    const [uncontrolled, setUncontrolled] = React.useState(defaultOpen)
    const open = controlled ?? uncontrolled
    const change = React.useCallback((next: boolean) => {
      if (controlled === undefined) setUncontrolled(next)
      onOpenChange?.(next)
    }, [controlled, onOpenChange])
    return [open, change] as const
  }

  return { OpenContext, useRootOpen, useOpen: () => React.useContext(OpenContext) }
}
