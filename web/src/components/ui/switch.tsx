import * as React from "react"
import { cn } from "@/lib/utils"
import { Switch as SwitchPrimitive } from "radix-ui"

function Switch({
  className,
  size = "default",
  ...props
}: React.ComponentProps<typeof SwitchPrimitive.Root> & {
  size?: "sm" | "default"
}) {
  return (
    <SwitchPrimitive.Root
      data-slot="switch"
      data-size={size}
      className={cn(
        "data-checked:bg-primary data-checked:bg-[image:var(--button-sheen)] data-checked:shadow-[0_0_0_1px_color-mix(in_oklch,var(--primary)_32%,transparent),0_2px_6px_-2px_color-mix(in_oklch,var(--primary)_40%,transparent)] data-unchecked:bg-input dark:data-unchecked:bg-input/85 hover:not-data-disabled:data-unchecked:bg-[color-mix(in_oklch,var(--foreground)_14%,var(--input))] focus-visible:border-ring focus-visible:shadow-focus aria-invalid:border-destructive aria-invalid:ring-3 aria-invalid:ring-destructive/20 dark:aria-invalid:border-destructive/50 dark:aria-invalid:ring-destructive/40 data-[size=default]:h-5 data-[size=default]:w-9 data-[size=sm]:h-4 data-[size=sm]:w-7 peer group/switch relative inline-flex shrink-0 items-center rounded-full border border-transparent outline-none transition-[background-color,box-shadow] duration-200 motion-reduce:transition-none after:absolute after:-inset-x-3 after:-inset-y-2 pointer-coarse:after:-inset-3 data-disabled:cursor-not-allowed data-disabled:opacity-50",
        className
      )}
      {...props}
    >
      <SwitchPrimitive.Thumb
        data-slot="switch-thumb"
        className="bg-background shadow-[0_1px_2.5px_color-mix(in_oklch,var(--foreground)_26%,transparent)] dark:data-unchecked:bg-foreground dark:data-checked:bg-primary-foreground pointer-events-none block rounded-full ring-0 transition-transform duration-200 ease-[cubic-bezier(0.34,1.3,0.64,1)] motion-reduce:transition-none group-data-[size=default]/switch:size-4 group-data-[size=sm]/switch:size-3 group-data-[size=default]/switch:data-checked:translate-x-[calc(100%-2px)] group-data-[size=sm]/switch:data-checked:translate-x-[calc(100%-2px)] group-data-[size=default]/switch:data-unchecked:translate-x-0 group-data-[size=sm]/switch:data-unchecked:translate-x-0"
      />
    </SwitchPrimitive.Root>
  )
}

export { Switch }
