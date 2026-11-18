import * as React from "react"
import { cn } from "@/lib/utils"

function Input({ className, type, ...props }: React.ComponentProps<"input">) {
  return (
    <input
      type={type}
      data-slot="input"
      className={cn(
        "field-control placeholder:text-muted-foreground file:text-foreground h-9 px-3 py-1 text-base file:inline-flex file:h-6 file:border-0 file:bg-transparent file:text-sm file:font-medium pointer-coarse:h-11 disabled:pointer-events-none disabled:opacity-55 md:text-sm",
        className
      )}
      {...props}
    />
  )
}

export { Input }
