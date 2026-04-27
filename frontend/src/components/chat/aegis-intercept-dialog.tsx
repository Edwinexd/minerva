/**
 * Just-in-time intercept dialog.
 *
 * Fires when the student presses Send and the analyzer has cached
 * suggestions for their draft. Shows the suggestions with two
 * buttons:
 *
 *   * **Revise**       -- close the dialog, leave the input
 *                          unchanged, return focus to the input.
 *                          The student can edit and try again
 *                          (a fresh analyzer call will re-run on
 *                          the next debounce tick).
 *   * **Send anyway**  -- close the dialog and actually send.
 *
 * Non-blocking by design (project brief: "Feedback should be
 * optional not blocking"). The dialog never traps the student;
 * "Send anyway" is always one click away. We intercept once per
 * Send press, so re-pressing Send after dismissing the dialog
 * does NOT loop -- the parent's `onSendAnyway` clears the cached
 * verdict, so the next Send goes through directly until a new
 * analysis with suggestions arrives.
 *
 * Keyboard:
 *   * Esc cancels (same as Revise -- the student didn't commit).
 *   * The default focus lands on Revise so an absent-mindedly
 *     dismissed dialog doesn't accidentally commit a send.
 */
import { useEffect, useRef } from "react"
import { useTranslation } from "react-i18next"
import { Button } from "@/components/ui/button"
import { AegisShield } from "@/components/icons/aegis-shield"
import type { AegisSuggestion } from "@/lib/types"
import { SuggestionRow } from "./aegis-feedback-panel"

interface AegisInterceptDialogProps {
  /** When false the dialog isn't mounted (no overlay, no a11y noise). */
  open: boolean
  /** Suggestions to surface; never rendered when empty (caller's job). */
  suggestions: AegisSuggestion[]
  /** Close + return focus to the chat input. The draft stays intact. */
  onRevise: () => void
  /** Close + actually send the message. */
  onSendAnyway: () => void
}

export function AegisInterceptDialog({
  open,
  suggestions,
  onRevise,
  onSendAnyway,
}: AegisInterceptDialogProps) {
  const { t } = useTranslation("student")
  const reviseRef = useRef<HTMLButtonElement | null>(null)

  // Esc closes via the Revise path. We intentionally don't bind Enter
  // to "Send anyway" -- the student should have to make an explicit
  // mouse/keyboard click on that specific button, otherwise we're
  // back to the same accident-prone "tap Enter to send" loop.
  useEffect(() => {
    if (!open) return
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onRevise()
    }
    window.addEventListener("keydown", onKey)
    return () => window.removeEventListener("keydown", onKey)
  }, [open, onRevise])

  // Land focus on Revise on open. The base-ui dialog primitive isn't
  // wired in for everything yet (we're using a hand-rolled overlay
  // here for this one-off), so we manage the ref directly.
  useEffect(() => {
    if (open) reviseRef.current?.focus()
  }, [open])

  if (!open) return null

  return (
    <div
      role="dialog"
      aria-modal="true"
      aria-labelledby="aegis-intercept-title"
      className="fixed inset-0 z-50 flex items-center justify-center bg-background/70 backdrop-blur-sm p-4"
      onClick={(e) => {
        // Click on backdrop = same as Revise. Inner card stops
        // propagation so dragging-to-select inside doesn't dismiss.
        if (e.target === e.currentTarget) onRevise()
      }}
    >
      <div className="bg-background border rounded-lg shadow-xl max-w-md w-full p-6 space-y-4">
        <div className="flex items-start gap-3">
          <AegisShield size={24} className="text-primary mt-0.5" aria-hidden="true" />
          <div className="space-y-1 flex-1">
            <h2
              id="aegis-intercept-title"
              className="text-base font-semibold"
            >
              {t("aegis.intercept.title")}
            </h2>
            <p className="text-xs text-muted-foreground">
              {t("aegis.intercept.body")}
            </p>
          </div>
        </div>

        <div className="space-y-2">
          {suggestions.map((s, i) => (
            <SuggestionRow key={i} suggestion={s} />
          ))}
        </div>

        <div className="flex gap-2 justify-end pt-2">
          <Button
            ref={reviseRef}
            type="button"
            variant="outline"
            onClick={onRevise}
          >
            {t("aegis.intercept.revise")}
          </Button>
          <Button type="button" onClick={onSendAnyway}>
            {t("aegis.intercept.sendAnyway")}
          </Button>
        </div>
      </div>
    </div>
  )
}
