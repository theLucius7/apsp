import type * as React from "react"
import { m } from "motion/react"
import { cn } from "@/lib/utils"

import { Loader2Icon } from "lucide-react"

const MotionLoader = m.create(Loader2Icon)

function Spinner({ className, ...props }: React.ComponentProps<typeof MotionLoader>) {
  return (
    <MotionLoader data-slot="spinner"
      role="status"
      aria-label="加载中"
      animate={{ rotate: 360 }}
      transition={{ repeat: Infinity, duration: 0.8, ease: "linear" }}
      className={cn("size-4", className)}
      {...props} />
  )
}

export { Spinner }
