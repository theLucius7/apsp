import * as React from "react"
import { cn } from "@/lib/utils"
import { Popover as PopoverPrimitive } from "radix-ui"
import { m } from "motion/react"

import { popoverMotion } from "@/lib/motion"

function Popover({
  ...props
}: React.ComponentProps<typeof PopoverPrimitive.Root>) {
  return <PopoverPrimitive.Root data-slot="popover" {...props} />
}

function PopoverTrigger({
  ...props
}: React.ComponentProps<typeof PopoverPrimitive.Trigger>) {
  return <PopoverPrimitive.Trigger data-slot="popover-trigger" {...props} />
}

// Like menus, popovers only animate in; a closed popover leaves the document at once.
function PopoverContent({
  className,
  align = "center",
  sideOffset = 4,
  children,
  ...props
}: React.ComponentProps<typeof PopoverPrimitive.Content>) {
  return (
    <PopoverPrimitive.Portal>
      <PopoverPrimitive.Content data-slot="popover-content" align={align} sideOffset={sideOffset} asChild {...props}>
        <m.div
          initial={popoverMotion.initial}
          animate={popoverMotion.animate}
          transition={popoverMotion.transition}
          className={cn(
            "bg-popover text-popover-foreground ring-foreground/10 z-50 flex max-h-(--radix-popover-content-available-height) w-72 origin-(--radix-popover-content-transform-origin) flex-col gap-4 overflow-y-auto rounded-lg p-4 text-sm shadow-md ring-1 outline-hidden",
            className
          )}
        >
          {children}
        </m.div>
      </PopoverPrimitive.Content>
    </PopoverPrimitive.Portal>
  )
}

export { Popover, PopoverContent, PopoverTrigger }
