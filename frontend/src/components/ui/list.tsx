import * as React from "react"

import { cn } from "@/lib/utils"
import { Skeleton } from "@/components/ui/skeleton"

/// One row of a simple settings list: content pushed to both edges,
/// hairline separator on every row but the last.
function ListRow({ className, ...props }: React.ComponentProps<"div">) {
  return (
    <div
      className={cn(
        "flex items-center justify-between py-2 border-b last:border-0",
        className,
      )}
      {...props}
    />
  )
}

/// Placeholder rows while a list query is in flight. `rows` should
/// roughly match the list's typical length so the layout doesn't jump
/// when the data lands.
function ListSkeleton({
  rows = 2,
  className,
}: {
  rows?: number
  className?: string
}) {
  return (
    <div className={cn("space-y-2", className)}>
      {Array.from({ length: rows }).map((_, i) => (
        <Skeleton key={i} className="h-10 w-full" />
      ))}
    </div>
  )
}

/// Centred muted text for a list that loaded successfully but is empty.
function ListEmpty({ className, ...props }: React.ComponentProps<"p">) {
  return (
    <p
      className={cn("text-sm text-muted-foreground py-4 text-center", className)}
      {...props}
    />
  )
}

export { ListRow, ListSkeleton, ListEmpty }
