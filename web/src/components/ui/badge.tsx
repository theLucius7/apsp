import * as React from "react"
import { cva, type VariantProps } from "class-variance-authority"
import { cn } from "@/lib/utils"
import { Slot } from "radix-ui"

const badgeVariants = cva(
  "group/badge inline-flex h-5 w-fit shrink-0 items-center justify-center gap-1 overflow-hidden rounded-full border border-transparent px-2 py-0.5 text-xs font-medium whitespace-nowrap ring-1 ring-inset ring-current/16 outline-none transition-colors duration-150 motion-reduce:transition-none has-data-[icon=inline-end]:pr-1.5 has-data-[icon=inline-start]:pl-1.5 focus-visible:border-ring focus-visible:shadow-focus aria-invalid:border-destructive aria-invalid:ring-destructive/20 dark:aria-invalid:ring-destructive/40 [&>svg]:pointer-events-none [&>svg]:size-3!",
  {
    variants: {
      variant: {
        default: "bg-primary text-primary-foreground [a]:hover:bg-primary/85",
        secondary: "bg-secondary text-secondary-foreground ring-border/70 [a]:hover:bg-secondary/80",
        success: "bg-success/10 text-success dark:bg-success/15 [a]:hover:bg-success/18",
        warning: "bg-warning/10 text-warning dark:bg-warning/15 [a]:hover:bg-warning/18",
        info: "bg-info/10 text-info dark:bg-info/15 [a]:hover:bg-info/18",
        destructive: "bg-destructive/10 text-destructive dark:bg-destructive/18 [a]:hover:bg-destructive/18 focus-visible:border-destructive/50",
        outline: "border-border text-foreground ring-0 [a]:hover:bg-muted [a]:hover:text-foreground",
        ghost: "ring-0 hover:bg-muted hover:text-muted-foreground dark:hover:bg-muted/50",
        link: "ring-0 text-primary underline-offset-4 hover:underline",
      },
    },
    defaultVariants: {
      variant: "default",
    },
  }
)

function Badge({
  className,
  variant = "default",
  asChild = false,
  ...props
}: React.ComponentProps<"span"> &
  VariantProps<typeof badgeVariants> & { asChild?: boolean }) {
  const Comp = asChild ? Slot.Root : "span"

  return (
    <Comp
      data-slot="badge"
      data-variant={variant}
      className={cn(badgeVariants({ variant }), className)}
      {...props}
    />
  )
}

export { Badge, badgeVariants }
