import { useApiErrorMessage } from "@/lib/use-api-error"
import { cn } from "@/lib/utils"

type ErrorTextProps = Omit<React.ComponentProps<"p">, "children"> & {
  error: unknown
}

/// Renders a failed mutation/query error through `useApiErrorMessage`.
/// Always `role="alert"`: the text appears in response to a user action,
/// so assistive tech has to be told without moving focus.
function ErrorText({ error, className, ...props }: ErrorTextProps) {
  const formatError = useApiErrorMessage()
  return (
    <p role="alert" className={cn("text-sm text-destructive", className)} {...props}>
      {formatError(error)}
    </p>
  )
}

export { ErrorText }
