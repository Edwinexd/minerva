import type { ReactNode } from "react"
import { Button } from "@/components/ui/button"
import { useCopyFeedback } from "@/lib/use-copy-feedback"

type SecretRevealCalloutProps = {
  title: ReactNode
  note: ReactNode
  /// Accessible name for the dismiss button. Omit when the visible
  /// label is already the accessible name.
  dismissAriaLabel?: string
  dismissLabel: string
  /// The secret itself. Shown once at creation and never again.
  value: string
  valueLabel: string
  copyLabel: string
  copiedLabel: string
  onDismiss: () => void
}

/// One-shot reveal of a freshly minted secret (integration key, invite
/// URL). The value sits in a readOnly input that selects itself on
/// focus, so a user without clipboard permission can still ctrl-C it,
/// and the copy confirmation is mirrored into an sr-only live region.
function SecretRevealCallout({
  title,
  note,
  dismissAriaLabel,
  dismissLabel,
  value,
  valueLabel,
  copyLabel,
  copiedLabel,
  onDismiss,
}: SecretRevealCalloutProps) {
  const { copiedKey, copy } = useCopyFeedback()

  return (
    <div className="mt-4 rounded-md border border-amber-300 bg-amber-50 p-3 text-sm dark:border-amber-700 dark:bg-amber-950/40">
      <div className="mb-2 flex items-center justify-between">
        <strong>{title}</strong>
        <button
          type="button"
          className="text-xs text-muted-foreground hover:underline"
          onClick={onDismiss}
          aria-label={dismissAriaLabel}
        >
          {dismissLabel}
        </button>
      </div>
      <p className="mb-2 text-xs text-muted-foreground">{note}</p>
      <div className="flex gap-2">
        <input
          readOnly
          value={value}
          aria-label={valueLabel}
          className="flex-1 rounded border bg-background px-2 py-1 font-mono text-xs"
          onFocus={(e) => e.currentTarget.select()}
        />
        <Button type="button" size="sm" variant="outline" onClick={() => void copy(value)}>
          {copiedKey ? copiedLabel : copyLabel}
        </Button>
        <output className="sr-only">{copiedKey ? copiedLabel : ""}</output>
      </div>
    </div>
  )
}

export { SecretRevealCallout }
