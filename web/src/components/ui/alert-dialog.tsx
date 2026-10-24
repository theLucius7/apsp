import * as React from "react"
import { cn } from "@/lib/utils"
import { AlertDialog as AlertDialogPrimitive } from "radix-ui"
import { AnimatePresence, m, useReducedMotion } from "motion/react"

import { dialogMotion, fadeMotion } from "@/lib/motion"
import { createPresence } from "@/components/ui/presence"
import { useReturnFocus } from "@/components/ui/return-focus"

import { Button } from "@/components/ui/button"

const presence = createPresence()

function AlertDialog({
  open: openProp,
  defaultOpen,
  onOpenChange,
  ...props
}: React.ComponentProps<typeof AlertDialogPrimitive.Root>) {
  const [open, setOpen] = presence.useRootOpen({ open: openProp, defaultOpen, onOpenChange })
  return (
    <presence.OpenContext.Provider value={open}>
      <AlertDialogPrimitive.Root data-slot="alert-dialog" open={open} onOpenChange={setOpen} {...props} />
    </presence.OpenContext.Provider>
  )
}

function AlertDialogTrigger({
  ...props
}: React.ComponentProps<typeof AlertDialogPrimitive.Trigger>) {
  return (
    <AlertDialogPrimitive.Trigger data-slot="alert-dialog-trigger" {...props} />
  )
}

function AlertDialogPortal({
  ...props
}: React.ComponentProps<typeof AlertDialogPrimitive.Portal>) {
  return (
    <AlertDialogPrimitive.Portal data-slot="alert-dialog-portal" {...props} />
  )
}

// A closing backdrop lets clicks through while it fades. Radix keeps an inline
// `pointer-events: auto` on it, so the closed state overrides with !important.
function AlertDialogOverlay({
  className,
  ...props
}: React.ComponentProps<typeof AlertDialogPrimitive.Overlay>) {
  const reduce = useReducedMotion()
  return (
    <AlertDialogPrimitive.Overlay data-slot="alert-dialog-overlay" forceMount asChild {...props}>
      <m.div
        {...fadeMotion}
        exit={reduce ? undefined : fadeMotion.exit}
        className={cn("bg-black/10 supports-backdrop-filter:backdrop-blur-xs fixed inset-0 z-50 data-[state=closed]:pointer-events-none!", className)}
      />
    </AlertDialogPrimitive.Overlay>
  )
}

// See DialogBox: a closing alert dialog is inert while its exit animation plays.
function AlertDialogBox(props: React.ComponentProps<typeof m.div>) {
  const open = presence.useOpen()
  // Visitors who ask for reduced motion see the dialog close at once.
  const reduce = useReducedMotion()
  return <m.div {...props} exit={reduce ? undefined : props.exit} inert={!open} />
}

function AlertDialogContent({
  className,
  size = "default",
  children,
  onCloseAutoFocus,
  onExitComplete,
  ...props
}: React.ComponentProps<typeof AlertDialogPrimitive.Content> & {
  size?: "default" | "sm"
  /** Called once the closing animation has finished and the dialog has left the page. */
  onExitComplete?: () => void
}) {
  const open = presence.useOpen()
  const returnFocus = useReturnFocus(open, onCloseAutoFocus)
  return (
    <AnimatePresence onExitComplete={onExitComplete}>
      {open && (
        <AlertDialogPortal forceMount>
          <AlertDialogOverlay />
          <AlertDialogPrimitive.Content
            data-slot="alert-dialog-content"
            data-size={size}
            forceMount
            asChild
            onCloseAutoFocus={returnFocus}
            {...props}
          >
            <AlertDialogBox
              {...dialogMotion}
              className={cn(
                "bg-popover text-popover-foreground ring-foreground/10 gap-4 rounded-xl p-4 ring-1 shadow-lg data-[size=default]:max-w-xs data-[size=sm]:max-w-xs data-[size=default]:sm:max-w-sm group/alert-dialog-content fixed top-1/2 left-1/2 z-50 grid w-full -translate-x-1/2 -translate-y-1/2 outline-none",
                className
              )}
            >
              {children}
            </AlertDialogBox>
          </AlertDialogPrimitive.Content>
        </AlertDialogPortal>
      )}
    </AnimatePresence>
  )
}

function AlertDialogHeader({
  className,
  ...props
}: React.ComponentProps<"div">) {
  return (
    <div
      data-slot="alert-dialog-header"
      className={cn("grid grid-rows-[auto_1fr] place-items-center gap-1.5 text-center has-data-[slot=alert-dialog-media]:grid-rows-[auto_auto_1fr] has-data-[slot=alert-dialog-media]:gap-x-4 sm:group-data-[size=default]/alert-dialog-content:place-items-start sm:group-data-[size=default]/alert-dialog-content:text-left sm:group-data-[size=default]/alert-dialog-content:has-data-[slot=alert-dialog-media]:grid-rows-[auto_1fr]", className)}
      {...props}
    />
  )
}

function AlertDialogFooter({
  className,
  ...props
}: React.ComponentProps<"div">) {
  return (
    <div
      data-slot="alert-dialog-footer"
      className={cn(
        "bg-muted/50 -mx-4 -mb-4 rounded-b-xl border-t p-4 flex flex-col-reverse gap-2 group-data-[size=sm]/alert-dialog-content:grid group-data-[size=sm]/alert-dialog-content:grid-cols-2 sm:flex-row sm:justify-end",
        className
      )}
      {...props}
    />
  )
}

function AlertDialogMedia({
  className,
  ...props
}: React.ComponentProps<"div">) {
  return (
    <div
      data-slot="alert-dialog-media"
      className={cn("bg-muted mb-2 inline-flex size-10 items-center justify-center rounded-md sm:group-data-[size=default]/alert-dialog-content:row-span-2 *:[svg:not([class*='size-'])]:size-6", className)}
      {...props}
    />
  )
}

function AlertDialogTitle({
  className,
  ...props
}: React.ComponentProps<typeof AlertDialogPrimitive.Title>) {
  return (
    <AlertDialogPrimitive.Title
      data-slot="alert-dialog-title"
      className={cn("text-base font-medium sm:group-data-[size=default]/alert-dialog-content:group-has-data-[slot=alert-dialog-media]/alert-dialog-content:col-start-2", className)}
      {...props}
    />
  )
}

function AlertDialogDescription({
  className,
  ...props
}: React.ComponentProps<typeof AlertDialogPrimitive.Description>) {
  return (
    <AlertDialogPrimitive.Description
      data-slot="alert-dialog-description"
      className={cn("text-muted-foreground *:[a]:hover:text-foreground text-sm text-balance md:text-pretty *:[a]:underline *:[a]:underline-offset-3", className)}
      {...props}
    />
  )
}

function AlertDialogAction({
  className,
  variant = "default",
  size = "default",
  ...props
}: React.ComponentProps<typeof AlertDialogPrimitive.Action> &
  Pick<React.ComponentProps<typeof Button>, "variant" | "size">) {
  return (
    <Button variant={variant} size={size} asChild>
      <AlertDialogPrimitive.Action
        data-slot="alert-dialog-action"
        className={cn("", className)}
        {...props}
      />
    </Button>
  )
}

function AlertDialogCancel({
  className,
  variant = "outline",
  size = "default",
  ...props
}: React.ComponentProps<typeof AlertDialogPrimitive.Cancel> &
  Pick<React.ComponentProps<typeof Button>, "variant" | "size">) {
  return (
    <Button variant={variant} size={size} asChild>
      <AlertDialogPrimitive.Cancel
        data-slot="alert-dialog-cancel"
        className={cn("", className)}
        {...props}
      />
    </Button>
  )
}

export {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogMedia,
  AlertDialogOverlay,
  AlertDialogPortal,
  AlertDialogTitle,
  AlertDialogTrigger,
}
