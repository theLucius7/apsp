import * as React from "react"
import { cn } from "@/lib/utils"

function Input({ className, type, ...props }: React.ComponentProps<"input">) {
  return (
    <input
      type={type}
      data-slot="input"
      className={cn(
        "bg-card/70 dark:bg-input/25 border-input hover:border-[color-mix(in_oklch,var(--primary)_30%,var(--input))] focus-visible:border-primary/70 focus-visible:shadow-focus aria-invalid:ring-destructive/20 dark:aria-invalid:ring-destructive/40 aria-invalid:border-destructive dark:aria-invalid:border-destructive/50 disabled:bg-input/50 dark:disabled:bg-input/80 h-9 pointer-coarse:h-11 rounded-[10px] border px-3 py-1 text-base transition-[border-color,box-shadow,background-color] duration-150 file:h-6 file:text-sm file:font-medium aria-invalid:ring-3 md:text-sm w-full min-w-0 outline-none file:inline-flex file:border-0 file:bg-transparent file:text-foreground placeholder:text-muted-foreground disabled:pointer-events-none disabled:cursor-not-allowed disabled:opacity-50",
        className
      )}
      {...props}
    />
  )
}

export { Input }
