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
        "group/native-select relative w-fit has-[select:disabled]:opacity-55",
        className
      )}
      data-slot="native-select-wrapper"
      data-size={size}
    >
      <select
        data-slot="native-select"
        data-size={size}
        className={cn(
          "field-control text-foreground selection:bg-primary selection:text-primary-foreground appearance-none py-1 pl-3 text-sm select-none disabled:pointer-events-none",
          // Native popups paint outside the page. Give every option an opaque,
          // paired surface/foreground, including callers using plain <option>.
          "[&_option]:bg-popover [&_option]:text-popover-foreground [&_optgroup]:bg-popover [&_optgroup]:text-popover-foreground",
          list
            ? "min-h-24 pr-2.5 [&>option]:rounded-sm [&>option]:px-1.5 [&>option]:py-1"
            : "h-9 pr-8 data-[size=sm]:h-8 data-[size=sm]:rounded-[9px] data-[size=sm]:py-0.5 pointer-coarse:h-11 pointer-coarse:data-[size=sm]:h-11"
        )}
        {...props}
      />
      {!list && <ChevronDownIcon className="text-muted-foreground pointer-events-none absolute top-1/2 right-2.5 size-4 -translate-y-1/2 select-none"
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
