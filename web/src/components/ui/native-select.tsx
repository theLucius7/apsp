import * as React from "react"
import { cn } from "@/lib/utils"

import { ChevronDownIcon } from "lucide-react"

type NativeSelectProps = Omit<React.ComponentProps<"select">, "size"> & {
  size?: "sm" | "default"
}

function NativeSelect({
  className,
  size = "default",
  ...props
}: NativeSelectProps) {
  // A multiple select is a list box: it shows several rows and has no drop-down arrow.
  const list = Boolean(props.multiple)
  return (
    <div
      className={cn(
        "group/native-select relative w-fit has-[select:disabled]:opacity-50",
        className
      )}
      data-slot="native-select-wrapper"
      data-size={size}
    >
      <select
        data-slot="native-select"
        data-size={size}
        className={cn(
          "border-input text-foreground placeholder:text-muted-foreground selection:bg-primary selection:text-primary-foreground bg-card/70 dark:bg-input/25 hover:border-[color-mix(in_oklch,var(--primary)_30%,var(--input))] focus-visible:border-primary/70 focus-visible:shadow-focus aria-invalid:ring-destructive/20 dark:aria-invalid:ring-destructive/40 aria-invalid:border-destructive dark:aria-invalid:border-destructive/50 w-full min-w-0 appearance-none rounded-[10px] border py-1 pl-3 text-sm transition-[border-color,box-shadow,background-color] duration-150 select-none aria-invalid:ring-3 outline-none disabled:pointer-events-none disabled:cursor-not-allowed",
          // Native popups paint outside the page. Give every option an opaque,
          // paired surface/foreground, including callers using plain <option>.
          "[&_option]:bg-popover [&_option]:text-popover-foreground [&_optgroup]:bg-popover [&_optgroup]:text-popover-foreground",
          list
            ? "min-h-24 pr-2.5 [&>option]:rounded-sm [&>option]:px-1.5 [&>option]:py-1"
            : "h-9 pointer-coarse:h-11 pr-8 data-[size=sm]:h-8 data-[size=sm]:rounded-[9px] data-[size=sm]:py-0.5"
        )}
        {...props}
      />
      {!list && <ChevronDownIcon className="text-muted-foreground top-1/2 right-2.5 size-4 -translate-y-1/2 pointer-events-none absolute select-none"
        aria-hidden="true"
        data-slot="native-select-icon" />}
    </div>
  )
}

function NativeSelectOption({
  className,
  ...props
}: React.ComponentProps<"option">) {
  return (
    <option
      data-slot="native-select-option"
      className={cn("bg-popover text-popover-foreground", className)}
      {...props}
    />
  )
}

function NativeSelectOptGroup({
  className,
  ...props
}: React.ComponentProps<"optgroup">) {
  return (
    <optgroup
      data-slot="native-select-optgroup"
      className={cn("bg-popover text-popover-foreground", className)}
      {...props}
    />
  )
}

export { NativeSelect, NativeSelectOptGroup, NativeSelectOption }
