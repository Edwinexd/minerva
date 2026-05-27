import { useEffect, useId, useRef, useState } from "react"
import { useTranslation } from "react-i18next"
import { XIcon } from "lucide-react"
import { Button } from "@/components/ui/button"
import { DataHandlingContent } from "@/components/data-handling"
import { useApiErrorMessage } from "@/lib/use-api-error"

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
  const { t } = useTranslation("student")
  const { t: tCommon } = useTranslation("common")
  const formatError = useApiErrorMessage()
  const [open, setOpen] = useState(false)
  const [submitting, setSubmitting] = useState(false)
  const [error, setError] = useState<unknown>(null)
  const titleId = useId()
  const dialogRef = useRef<HTMLDialogElement>(null)

  const handleClose = () => setOpen(false)

  const handleAgree = async () => {
    setSubmitting(true)
    setError(null)
    try {
      await onAcknowledge()
      setOpen(false)
    } catch (e) {
      setError(e instanceof Error ? e : new Error(t("privacy.acknowledgeFailed")))
    } finally {
      setSubmitting(false)
    }
  }

  // Drive the native <dialog> as a true modal. showModal() puts it in the top
  // layer with a ::backdrop and provides focus trapping, Escape-to-close and
  // focus restoration natively, so no hand-rolled focus management is needed.
  useEffect(() => {
    const el = dialogRef.current
    if (!el) return
    if (open && !el.open) el.showModal()
    else if (!open && el.open) el.close()
  }, [open])

  // Light-dismiss: a click landing on the dialog element itself is a click on
  // the ::backdrop (all content lives in child nodes), so close. Registered
  // imperatively to keep it off the JSX and to ignore clicks mid-submit.
  useEffect(() => {
    const el = dialogRef.current
    if (!el) return
    const onBackdropClick = (e: MouseEvent) => {
      if (!submitting && e.target === el) setOpen(false)
    }
    el.addEventListener("click", onBackdropClick)
    return () => el.removeEventListener("click", onBackdropClick)
  }, [submitting])

  return (
    <>
      <div className="flex flex-wrap items-center justify-between gap-2 rounded-md border border-amber-300 bg-amber-50 px-3 py-2 text-sm dark:border-amber-800 dark:bg-amber-950/40">
        <span className="text-amber-900 dark:text-amber-200">
          {t("privacy.bannerText")}
        </span>
        <Button size="sm" onClick={() => setOpen(true)}>
          {t("privacy.reviewButton")}
        </Button>
      </div>

      <dialog
        ref={dialogRef}
        aria-labelledby={titleId}
        onCancel={(e) => {
          // Block Escape-to-close while the acknowledgement is saving.
          if (submitting) e.preventDefault()
        }}
        onClose={() => setOpen(false)}
        className="m-auto flex max-h-[90vh] w-[calc(100%-2rem)] max-w-2xl flex-col overflow-hidden rounded-xl border-0 bg-popover p-0 text-popover-foreground ring-1 ring-foreground/10 shadow-lg backdrop:bg-black/40"
      >
        {open && (
          <>
            <div className="flex items-center justify-between border-b px-6 py-4">
              <h2 id={titleId} className="text-lg font-semibold">{t("privacy.dialogTitle")}</h2>
              <button
                onClick={handleClose}
                disabled={submitting}
                className="rounded p-1 text-muted-foreground hover:text-foreground disabled:opacity-50"
                aria-label={t("privacy.closeLabel")}
              >
                <XIcon aria-hidden className="h-4 w-4" />
              </button>
            </div>
            <div className="flex-1 overflow-y-auto px-6 py-4">
              <DataHandlingContent />
            </div>
            <div className="flex flex-col-reverse gap-2 border-t bg-muted/50 px-6 py-3 sm:flex-row sm:justify-end">
              {error !== null && (
                <p role="alert" className="mr-auto self-center text-sm text-destructive">{formatError(error)}</p>
              )}
              <Button variant="outline" onClick={handleClose} disabled={submitting}>
                {tCommon("actions.close")}
              </Button>
              <Button onClick={handleAgree} disabled={submitting}>
                {submitting ? t("privacy.savingButton") : t("privacy.agreeButton")}
              </Button>
            </div>
          </>
        )}
      </dialog>
    </>
  )
}
