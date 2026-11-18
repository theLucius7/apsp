import * as React from "react"
import { cn } from "@/lib/utils"
import { Checkbox as CheckboxPrimitive } from "radix-ui"

import { CheckIcon, MinusIcon } from "lucide-react"

function Checkbox({
  className,
  ...props
}: React.ComponentProps<typeof CheckboxPrimitive.Root>) {
  return (
    <CheckboxPrimitive.Root
      data-slot="checkbox"
      className={cn(
        "group/checkbox border-input bg-background/85 dark:bg-background/55 hover:not-disabled:not-data-checked:border-[color-mix(in_oklch,var(--primary)_40%,var(--input))] data-[state=indeterminate]:border-primary data-[state=indeterminate]:bg-primary data-[state=indeterminate]:text-primary-foreground data-checked:border-primary data-checked:bg-primary data-checked:text-primary-foreground dark:data-checked:bg-primary data-checked:shadow-[0_1px_3px_-1px_color-mix(in_oklch,var(--primary)_45%,transparent)] focus-visible:border-ring focus-visible:shadow-focus aria-invalid:border-destructive aria-invalid:aria-checked:border-primary aria-invalid:ring-3 aria-invalid:ring-destructive/20 dark:aria-invalid:border-destructive/50 dark:aria-invalid:ring-destructive/40 peer relative flex size-4 shrink-0 items-center justify-center rounded-[5px] border outline-none transition-[background-color,border-color,box-shadow] duration-150 motion-reduce:transition-none after:absolute after:-inset-x-3 after:-inset-y-2 pointer-coarse:after:-inset-3 disabled:cursor-not-allowed disabled:opacity-50",
        className
      )}
      {...props}
    >
      <CheckboxPrimitive.Indicator
        data-slot="checkbox-indicator"
        className="[&>svg]:size-3.5 grid place-content-center text-current motion-safe:animate-[check-pop_160ms_ease-out]"
      >
        <CheckIcon className="group-data-[state=indeterminate]/checkbox:hidden" />
        <MinusIcon className="hidden group-data-[state=indeterminate]/checkbox:block" />
      </CheckboxPrimitive.Indicator>
    </CheckboxPrimitive.Root>
  )
}

export { Checkbox }
