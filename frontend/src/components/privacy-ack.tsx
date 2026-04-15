import { useState } from "react"
import { Button } from "@/components/ui/button"
import { DataHandlingContent } from "@/components/data-handling"

/**
 * Blocking banner + modal shown above the chat input for students who have
 * not yet acknowledged the in-app data-handling disclosure. Students can
 * still read conversations; only sending new messages is gated. The modal
 * contains the same disclosure text as the standalone /data-handling page.
 */
export function PrivacyAckBanner({
  onAcknowledge,
}: {
  onAcknowledge: () => Promise<void>
}) {
  const [open, setOpen] = useState(false)
  const [submitting, setSubmitting] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const handleAgree = async () => {
    setSubmitting(true)
    setError(null)
    try {
      await onAcknowledge()
      setOpen(false)
    } catch (e) {
      setError(e instanceof Error ? e.message : "Failed to record acknowledgment")
    } finally {
      setSubmitting(false)
    }
  }

  return (
    <>
      <div className="flex flex-wrap items-center justify-between gap-2 rounded-md border border-amber-300 bg-amber-50 px-3 py-2 text-sm dark:border-amber-800 dark:bg-amber-950/40">
        <span className="text-amber-900 dark:text-amber-200">
          Before sending messages, please review how Minerva handles your data.
        </span>
        <Button size="sm" onClick={() => setOpen(true)}>
          Review & agree
        </Button>
      </div>

      {open && (
        <div
          className="fixed inset-0 z-50 flex items-center justify-center bg-black/40 p-4"
          role="dialog"
          aria-modal="true"
          onClick={(e) => { if (e.target === e.currentTarget) setOpen(false) }}
        >
          <div className="relative flex max-h-[90vh] w-full max-w-2xl flex-col overflow-hidden rounded-xl bg-popover text-popover-foreground ring-1 ring-foreground/10 shadow-lg">
            <div className="flex items-center justify-between border-b px-6 py-4">
              <h2 className="text-lg font-semibold">How Minerva handles your data</h2>
              <button
                onClick={() => setOpen(false)}
                className="rounded p-1 text-muted-foreground hover:text-foreground"
                aria-label="Close"
              >
                ✕
              </button>
            </div>
            <div className="flex-1 overflow-y-auto px-6 py-4">
              <DataHandlingContent />
            </div>
            <div className="flex flex-col-reverse gap-2 border-t bg-muted/50 px-6 py-3 sm:flex-row sm:justify-end">
              {error && (
                <p className="mr-auto self-center text-sm text-destructive">{error}</p>
              )}
              <Button variant="outline" onClick={() => setOpen(false)} disabled={submitting}>
                Close
              </Button>
              <Button onClick={handleAgree} disabled={submitting}>
                {submitting ? "Saving..." : "I understand and agree"}
              </Button>
            </div>
          </div>
        </div>
      )}
    </>
  )
}
