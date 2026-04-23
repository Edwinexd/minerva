import { useCallback, useEffect, useId, useRef, useState } from "react"
import { useTranslation } from "react-i18next"
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
  const dialogRef = useRef<HTMLDivElement>(null)
  const previousFocusRef = useRef<HTMLElement | null>(null)

  const handleClose = useCallback(() => setOpen(false), [])

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

  // Focus management + Escape + Tab trap while the dialog is open.
  useEffect(() => {
    if (!open) return

    previousFocusRef.current = document.activeElement as HTMLElement | null

    const getFocusable = () => {
      const root = dialogRef.current
      if (!root) return [] as HTMLElement[]
      return Array.from(
        root.querySelectorAll<HTMLElement>(
          'a[href], button:not([disabled]), textarea:not([disabled]), input:not([disabled]), select:not([disabled]), [tabindex]:not([tabindex="-1"])',
        ),
      ).filter((el) => !el.hasAttribute("aria-hidden"))
    }

    // Focus the first interactive element so keyboard users land inside.
    const focusables = getFocusable()
    focusables[0]?.focus()

    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key === "Escape" && !submitting) {
        e.stopPropagation()
        handleClose()
        return
      }
      if (e.key === "Tab") {
        const nodes = getFocusable()
        if (nodes.length === 0) return
        const first = nodes[0]
        const last = nodes[nodes.length - 1]
        const active = document.activeElement as HTMLElement | null
        if (e.shiftKey) {
          if (active === first || !dialogRef.current?.contains(active)) {
            e.preventDefault()
            last.focus()
          }
        } else {
          if (active === last) {
            e.preventDefault()
            first.focus()
          }
        }
      }
    }

    document.addEventListener("keydown", onKeyDown)
    return () => {
      document.removeEventListener("keydown", onKeyDown)
      // Restore focus to the trigger when the dialog closes.
      previousFocusRef.current?.focus()
    }
  }, [open, submitting, handleClose])

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

      {open && (
        <div
          className="fixed inset-0 z-50 flex items-center justify-center bg-black/40 p-4"
          onClick={(e) => { if (e.target === e.currentTarget && !submitting) setOpen(false) }}
        >
          <div
            ref={dialogRef}
            role="dialog"
            aria-modal="true"
            aria-labelledby={titleId}
            className="relative flex max-h-[90vh] w-full max-w-2xl flex-col overflow-hidden rounded-xl bg-popover text-popover-foreground ring-1 ring-foreground/10 shadow-lg"
          >
            <div className="flex items-center justify-between border-b px-6 py-4">
              <h2 id={titleId} className="text-lg font-semibold">{t("privacy.dialogTitle")}</h2>
              <button
                onClick={handleClose}
                disabled={submitting}
                className="rounded p-1 text-muted-foreground hover:text-foreground disabled:opacity-50"
                aria-label={t("privacy.closeLabel")}
              >
                ✕
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
          </div>
        </div>
      )}
    </>
  )
}
