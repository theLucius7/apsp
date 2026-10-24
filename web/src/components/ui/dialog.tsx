import * as React from "react"
import { cn } from "@/lib/utils"
import { Dialog as DialogPrimitive } from "radix-ui"
import { AnimatePresence, m, useReducedMotion } from "motion/react"

import { dialogMotion, fadeMotion, sheetMotion } from "@/lib/motion"
import { createPresence } from "@/components/ui/presence"
import { useReturnFocus } from "@/components/ui/return-focus"

import { Button } from "@/components/ui/button"
import { XIcon } from "lucide-react"

const presence = createPresence()

function Dialog({
  open: openProp,
  defaultOpen,
  onOpenChange,
  ...props
}: React.ComponentProps<typeof DialogPrimitive.Root>) {
  const [open, setOpen] = presence.useRootOpen({ open: openProp, defaultOpen, onOpenChange })
  return (
    <presence.OpenContext.Provider value={open}>
      <DialogPrimitive.Root data-slot="dialog" open={open} onOpenChange={setOpen} {...props} />
    </presence.OpenContext.Provider>
  )
}

function DialogTrigger({
  ...props
}: React.ComponentProps<typeof DialogPrimitive.Trigger>) {
  return <DialogPrimitive.Trigger data-slot="dialog-trigger" {...props} />
}

function DialogPortal({
  ...props
}: React.ComponentProps<typeof DialogPrimitive.Portal>) {
  return <DialogPrimitive.Portal data-slot="dialog-portal" {...props} />
}

function DialogClose({
  ...props
}: React.ComponentProps<typeof DialogPrimitive.Close>) {
  return <DialogPrimitive.Close data-slot="dialog-close" {...props} />
}

// A closing backdrop lets clicks through while it fades. Radix keeps an inline
// `pointer-events: auto` on it, so the closed state overrides with !important.
function DialogOverlay({
  className,
  ...props
}: React.ComponentProps<typeof DialogPrimitive.Overlay>) {
  const reduce = useReducedMotion()
  return (
    <DialogPrimitive.Overlay data-slot="dialog-overlay" forceMount asChild {...props}>
      <m.div
        {...fadeMotion}
        exit={reduce ? undefined : fadeMotion.exit}
        className={cn("bg-black/10 supports-backdrop-filter:backdrop-blur-xs fixed inset-0 isolate z-50 data-[state=closed]:pointer-events-none!", className)}
      />
    </DialogPrimitive.Overlay>
  )
}

// The animated box keeps rendering while it exits, so a closing dialog can step
// out of the page at once: inert, it no longer takes focus or clicks, and
// assistive technology skips it.
function DialogBox(props: React.ComponentProps<typeof m.div>) {
  const open = presence.useOpen()
  // Visitors who ask for reduced motion see the dialog close at once.
  const reduce = useReducedMotion()
  return <m.div {...props} exit={reduce ? undefined : props.exit} inert={!open} />
}

function DialogContent({
  className,
  children,
  showCloseButton = true,
  closeLabel = "关闭",
  overlayClassName,
  onCloseAutoFocus,
  onExitComplete,
  placement = "center",
  ...props
}: React.ComponentProps<typeof DialogPrimitive.Content> & {
  showCloseButton?: boolean
  closeLabel?: string
  overlayClassName?: string
  /** Called once the closing animation has finished and the dialog has left the page. */
  onExitComplete?: () => void
  /** "bottom" shows the dialog as a drawer along the bottom edge, for phones. */
  placement?: "center" | "bottom"
}) {
  const open = presence.useOpen()
  const returnFocus = useReturnFocus(open, onCloseAutoFocus)
  return (
    <AnimatePresence onExitComplete={onExitComplete}>
      {open && (
        <DialogPortal forceMount>
          <DialogOverlay className={overlayClassName} />
          <DialogPrimitive.Content data-slot="dialog-content" forceMount asChild onCloseAutoFocus={returnFocus} {...props}>
            <DialogBox
              {...(placement === "bottom" ? sheetMotion : dialogMotion)}
              data-placement={placement}
              className={cn(
                placement === "bottom"
                  ? "bg-popover text-popover-foreground ring-foreground/10 grid max-h-[85dvh] gap-4 overflow-y-auto rounded-t-xl p-4 pb-[max(1rem,env(safe-area-inset-bottom))] text-sm ring-1 shadow-lg fixed inset-x-0 bottom-0 z-50 w-full outline-none"
                  : "bg-popover text-popover-foreground ring-foreground/10 grid max-w-[calc(100%-2rem)] gap-4 rounded-xl p-4 text-sm ring-1 shadow-lg sm:max-w-sm fixed top-1/2 left-1/2 z-50 w-full -translate-x-1/2 -translate-y-1/2 outline-none",
                className
              )}
            >
              {children}
              {showCloseButton && (
                <DialogPrimitive.Close data-slot="dialog-close" asChild>
                  <Button variant="ghost" className="absolute top-2 right-2" size="icon-sm" aria-label={closeLabel}>
                    <XIcon />
                  </Button>
                </DialogPrimitive.Close>
              )}
            </DialogBox>
          </DialogPrimitive.Content>
        </DialogPortal>
      )}
    </AnimatePresence>
  )
}

function DialogHeader({ className, ...props }: React.ComponentProps<"div">) {
  return (
    <div
      data-slot="dialog-header"
      className={cn("gap-2 flex flex-col", className)}
      {...props}
    />
  )
}

function DialogFooter({
  className,
  showCloseButton = false,
  children,
  ...props
}: React.ComponentProps<"div"> & {
  showCloseButton?: boolean
}) {
  return (
    <div
      data-slot="dialog-footer"
      className={cn(
        "bg-muted/50 -mx-4 -mb-4 rounded-b-xl border-t p-4 flex flex-col-reverse gap-2 sm:flex-row sm:justify-end",
        className
      )}
      {...props}
    >
      {children}
      {showCloseButton && (
        <DialogPrimitive.Close asChild>
          <Button variant="outline">关闭</Button>
        </DialogPrimitive.Close>
      )}
    </div>
  )
}

function DialogTitle({
  className,
  ...props
}: React.ComponentProps<typeof DialogPrimitive.Title>) {
  return (
    <DialogPrimitive.Title
      data-slot="dialog-title"
      className={cn("text-base leading-none font-medium", className)}
      {...props}
    />
  )
}

function DialogDescription({
  className,
  ...props
}: React.ComponentProps<typeof DialogPrimitive.Description>) {
  return (
    <DialogPrimitive.Description
      data-slot="dialog-description"
      className={cn("text-muted-foreground *:[a]:hover:text-foreground text-sm *:[a]:underline *:[a]:underline-offset-3", className)}
      {...props}
    />
  )
}

export {
  Dialog,
  DialogClose,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogOverlay,
  DialogPortal,
  DialogTitle,
  DialogTrigger,
}
