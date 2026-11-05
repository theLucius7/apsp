import type * as React from "react"
import { cn } from "@/lib/utils"

import { Loader2Icon } from "lucide-react"

// A CSS rotation runs on the compositor; a script-driven one would wake the
// main thread every frame for each spinner on screen.
function Spinner({ className, ...props }: React.ComponentProps<typeof Loader2Icon>) {
  return (
    <Loader2Icon data-slot="spinner"
      role="status"
      aria-label="加载中"
      className={cn("size-4 animate-spin [animation-duration:800ms] motion-reduce:animate-none", className)}
      {...props} />
  )
}

export { Spinner }
