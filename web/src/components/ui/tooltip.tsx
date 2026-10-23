import * as React from "react"
import { cn } from "@/lib/utils"
import { Tooltip as TooltipPrimitive } from "radix-ui"
import { AnimatePresence, m } from "motion/react"

import { popoverMotion } from "@/lib/motion"
import { createPresence } from "@/components/ui/presence"

function TooltipProvider({
  delayDuration = 0,
  ...props
}: React.ComponentProps<typeof TooltipPrimitive.Provider>) {
  return (
    <TooltipPrimitive.Provider
      data-slot="tooltip-provider"
      delayDuration={delayDuration}
      {...props}
    />
  )
}

const presence = createPresence()

function Tooltip({
  open: openProp,
  defaultOpen,
  onOpenChange,
  ...props
}: React.ComponentProps<typeof TooltipPrimitive.Root>) {
  const [open, setOpen] = presence.useRootOpen({ open: openProp, defaultOpen, onOpenChange })
  return (
    <presence.OpenContext.Provider value={open}>
      <TooltipPrimitive.Root data-slot="tooltip" open={open} onOpenChange={setOpen} {...props} />
    </presence.OpenContext.Provider>
  )
}

function TooltipTrigger({
  ...props
}: React.ComponentProps<typeof TooltipPrimitive.Trigger>) {
  return <TooltipPrimitive.Trigger data-slot="tooltip-trigger" {...props} />
}

function TooltipContent({
  className,
  sideOffset = 0,
  children,
  ...props
}: React.ComponentProps<typeof TooltipPrimitive.Content>) {
  const open = presence.useOpen()
  return (
    <AnimatePresence>
      {open && (
        <TooltipPrimitive.Portal forceMount>
          <TooltipPrimitive.Content
            data-slot="tooltip-content"
            sideOffset={sideOffset}
            forceMount
            asChild
            {...props}
          >
            <m.div
              {...popoverMotion}
              className={cn(
                "inline-flex items-center gap-1.5 rounded-md px-3 py-1.5 text-xs has-data-[slot=kbd]:pr-1.5 **:data-[slot=kbd]:relative **:data-[slot=kbd]:isolate **:data-[slot=kbd]:z-50 **:data-[slot=kbd]:rounded-sm z-50 w-fit max-w-xs origin-(--radix-tooltip-content-transform-origin) bg-foreground text-background",
                className
              )}
            >
              {children}
              <TooltipPrimitive.Arrow className="size-2.5 translate-y-[calc(-50%_-_2px)] rotate-45 rounded-[2px] z-50 bg-foreground fill-foreground" />
            </m.div>
          </TooltipPrimitive.Content>
        </TooltipPrimitive.Portal>
      )}
    </AnimatePresence>
  )
}

export { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger }
