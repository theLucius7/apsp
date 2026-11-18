import * as React from "react"
import { cva, type VariantProps } from "class-variance-authority"
import { cn } from "@/lib/utils"
import { Slot } from "radix-ui"

// Sizes step by 4px (28 / 32 / 36 / 40) on fine pointers and meet 44px+ on coarse
// touch pointers. Every variant answers interaction with one calm language: subtle
// border/surface shift on hover, a slight composed press, and a unified focus glow.
const buttonVariants = cva(
  "group/button relative inline-flex shrink-0 items-center justify-center whitespace-nowrap rounded-[10px] border border-transparent bg-clip-padding text-[13px] font-medium tracking-[0.005em] select-none outline-none transition-[color,background-color,border-color,box-shadow,transform,filter] duration-150 ease-out focus-visible:border-ring focus-visible:shadow-focus active:not-aria-[haspopup]:scale-[0.985] disabled:pointer-events-none disabled:opacity-45 disabled:shadow-none aria-invalid:border-destructive aria-invalid:ring-3 aria-invalid:ring-destructive/20 dark:aria-invalid:ring-destructive/40 dark:aria-invalid:border-destructive/50 motion-reduce:transition-none motion-reduce:active:scale-100 [&_svg]:pointer-events-none [&_svg]:shrink-0 [&_svg:not([class*='size-'])]:size-4",
  {
    variants: {
      variant: {
        default: "bg-primary bg-[image:var(--button-sheen)] text-primary-foreground shadow-button hover:shadow-button-hover hover:brightness-[1.04] focus-visible:shadow-[var(--button-shadow),var(--focus-glow)]",
        outline: "border-border bg-card text-foreground shadow-soft hover:border-[color-mix(in_oklch,var(--primary)_32%,var(--border))] hover:bg-muted/65 hover:text-foreground aria-expanded:border-[color-mix(in_oklch,var(--primary)_32%,var(--border))] aria-expanded:bg-muted/75",
        secondary: "border-border/60 bg-secondary text-secondary-foreground hover:bg-[color-mix(in_oklch,var(--secondary),var(--foreground)_5%)] aria-expanded:bg-[color-mix(in_oklch,var(--secondary),var(--foreground)_6%)]",
        ghost: "text-foreground/85 hover:bg-[color-mix(in_oklch,var(--foreground)_6%,transparent)] hover:text-foreground aria-expanded:bg-[color-mix(in_oklch,var(--foreground)_8%,transparent)] aria-expanded:text-foreground",
        destructive: "border-destructive/20 bg-destructive/10 text-destructive hover:border-destructive/30 hover:bg-destructive/16 focus-visible:border-destructive/50 focus-visible:shadow-[0_0_0_3px_color-mix(in_oklch,var(--destructive)_22%,transparent)] dark:border-destructive/28 dark:bg-destructive/18 dark:hover:bg-destructive/26",
        link: "text-primary underline-offset-4 hover:underline active:scale-100",
      },
      size: {
        default: "h-9 gap-1.5 px-3.5 pointer-coarse:h-11 has-data-[icon=inline-end]:pr-3 has-data-[icon=inline-start]:pl-3",
        xs: "h-7 gap-1 rounded-lg px-2.5 text-xs pointer-coarse:h-11 has-data-[icon=inline-end]:pr-2 has-data-[icon=inline-start]:pl-2 [&_svg:not([class*='size-'])]:size-3.5",
        sm: "h-8 gap-1.5 rounded-[9px] px-3 text-[13px] pointer-coarse:h-11 has-data-[icon=inline-end]:pr-2.5 has-data-[icon=inline-start]:pl-2.5 [&_svg:not([class*='size-'])]:size-3.5",
        lg: "h-10 gap-2 rounded-xl px-5 text-sm pointer-coarse:h-12 has-data-[icon=inline-end]:pr-4 has-data-[icon=inline-start]:pl-4",
        icon: "size-9 pointer-coarse:size-11",
        "icon-xs": "size-7 rounded-lg pointer-coarse:size-11 [&_svg:not([class*='size-'])]:size-3.5",
        "icon-sm": "size-8 rounded-[9px] pointer-coarse:size-11",
        "icon-lg": "size-10 rounded-xl pointer-coarse:size-12",
      },
    },
    defaultVariants: {
      variant: "default",
      size: "default",
    },
  }
)

function Button({
  className,
  variant = "default",
  size = "default",
  asChild = false,
  ...props
}: React.ComponentProps<"button"> &
  VariantProps<typeof buttonVariants> & {
    asChild?: boolean
  }) {
  const Comp = asChild ? Slot.Root : "button"

  return (
    <Comp
      data-slot="button"
      data-variant={variant}
      data-size={size}
      className={cn(buttonVariants({ variant, size, className }))}
      {...props}
    />
  )
}

export { Button, buttonVariants }
