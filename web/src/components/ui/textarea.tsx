import * as React from "react"
import { cn } from "@/lib/utils"

function Textarea({ className, ...props }: React.ComponentProps<"textarea">) {
  return (
    <textarea
      data-slot="textarea"
      className={cn(
        "field-control placeholder:text-muted-foreground flex field-sizing-content min-h-16 px-3 py-2 text-base pointer-coarse:py-2.5 disabled:opacity-55 md:text-sm",
        className
      )}
      {...props}
    />
  )
}

export { Textarea }
