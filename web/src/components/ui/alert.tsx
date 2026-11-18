import * as React from "react"
import { cva, type VariantProps } from "class-variance-authority"
import { cn } from "@/lib/utils"

const alertVariants = cva("group/alert relative grid w-full gap-1 rounded-xl border px-3.5 py-2.5 text-left text-[13px] leading-relaxed has-data-[slot=alert-action]:relative has-data-[slot=alert-action]:pr-18 has-[>svg]:grid-cols-[auto_1fr] has-[>svg]:gap-x-2.5 *:[svg]:row-span-2 *:[svg]:translate-y-0.5 *:[svg]:text-current *:[svg:not([class*='size-'])]:size-4", {
  variants: {
    variant: {
      default: "bg-card text-card-foreground border-border/85",
      destructive: "border-destructive/30 bg-destructive/6 text-destructive dark:border-destructive/35 dark:bg-destructive/12 *:data-[slot=alert-description]:text-destructive *:[svg]:text-current",
      success: "border-success/28 bg-success/6 text-foreground dark:border-success/35 dark:bg-success/10 *:[svg]:text-success",
      warning: "border-warning/30 bg-warning/7 text-foreground dark:border-warning/35 dark:bg-warning/10 *:[svg]:text-warning",
      info: "border-info/25 bg-info/6 text-foreground dark:border-info/30 dark:bg-info/10 *:[svg]:text-info",
    },
  },
  defaultVariants: {
    variant: "default",
  },
})

function Alert({
  className,
  variant,
  ...props
}: React.ComponentProps<"div"> & VariantProps<typeof alertVariants>) {
  return (
    <div
      data-slot="alert"
      role="alert"
      className={cn(alertVariants({ variant }), className)}
      {...props}
    />
  )
}

function AlertTitle({ className, ...props }: React.ComponentProps<"div">) {
  return (
    <div
      data-slot="alert-title"
      className={cn(
        "font-medium group-has-[>svg]/alert:col-start-2 [&_a]:underline [&_a]:underline-offset-3 [&_a]:hover:text-foreground",
        className
      )}
      {...props}
    />
  )
}

function AlertDescription({
  className,
  ...props
}: React.ComponentProps<"div">) {
  return (
    <div
      data-slot="alert-description"
      className={cn(
        "text-muted-foreground text-[13px] leading-relaxed text-balance md:text-pretty [&_p:not(:last-child)]:mb-3 [&_a]:underline [&_a]:underline-offset-3 [&_a]:hover:text-foreground",
        className
      )}
      {...props}
    />
  )
}

function AlertAction({ className, ...props }: React.ComponentProps<"div">) {
  return (
    <div
      data-slot="alert-action"
      className={cn("absolute top-2.5 right-2.5", className)}
      {...props}
    />
  )
}

export { Alert, AlertTitle, AlertDescription, AlertAction }
