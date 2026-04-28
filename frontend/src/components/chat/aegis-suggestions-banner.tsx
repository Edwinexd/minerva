/**
 * Above-the-input banner that surfaces Aegis suggestions for the
 * student's current draft. Replaces the previous inline amber
 * sentence under the Send button; the same information now sits
 * visibly above the input where Slack/iMessage place "replying to"
 * or "attached file" chips, so the student SEES it without having
 * to glance away from the input.
 *
 * Two-stage UX, post-pilot rework:
 *
 *   1. Compact pill: "Aegis has N ideas" + Review button. The pill never
 *      auto-rewrites or auto-sends; it just announces and offers.
 *
 *   2. Expanded review tray (after the student clicks Review):
 *      every active suggestion appears as a checkbox row showing
 *      its kind tag + headline. The student picks which ones to
 *      fold in (default all), then clicks Preview to see the
 *      rewritten draft. The rewrite is shown READ-ONLY; the only
 *      decisions left are "Apply to my draft" (replaces the input
 *      box; the student still has to press Send themselves) or
 *      "Cancel" (collapses the tray, leaves the original draft
 *      untouched).
 *
 * The rewrite-and-auto-send flow this replaces was the single
 * loudest piece of pilot feedback: testers said "Use ideas" was
 * unclear about which ideas it was using, and felt uncomfortable
 * that the rewritten prompt was sent without them having seen it.
 * The new flow forces engagement; the student picks what to apply,
 * sees the result, and presses Send themselves.
 *
 * Visual states beyond compact/expanded:
 *   * `blocked` ; the student pressed Send while suggestions were
 *                   active. Rose border in compact form, copy
 *                   changes to "Press Send again to send as-is".
 *   * `working` ; a /aegis/rewrite request is in flight. Preview
 *                   button shows "Rewriting..." + disabled.
 *
 * The dismiss X collapses the banner for THIS draft only; new
 * input regenerates a fresh verdict and the banner can return.
 */
import { useEffect, useMemo, useState } from "react"
import { useTranslation } from "react-i18next"
import { ChevronDown, Wand2, X } from "lucide-react"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { Checkbox } from "@/components/ui/checkbox"
import { AegisShieldFilled } from "@/components/icons/aegis-shield-filled"
import { cn } from "@/lib/utils"
import type { AegisSuggestion } from "@/lib/types"

interface AegisSuggestionsBannerProps {
  /**
   * The active suggestions for the current draft. Drives the
   * count in the compact header and the per-row checkbox list in
   * the expanded tray.
   */
  suggestions: AegisSuggestion[]
  /** True when the student pressed Send while suggestions were active. */
  blocked: boolean
  /** True while a rewrite request is in flight. */
  working: boolean
  /**
   * Ask the parent to call the rewrite endpoint with the chosen
   * subset. Resolves to the rewritten draft text, or null on
   * failure (parent decides how to surface that; we just clear
   * any stale preview).
   */
  onPreview: (selected: AegisSuggestion[]) => Promise<string | null>
  /**
   * Replace the chat input with `rewritten`. Does NOT trigger a
   * Send; the student presses Send themselves on the next pass.
   * The banner closes itself once the parent has accepted.
   */
  onApply: (rewritten: string) => void
  /** Collapse the banner for the current draft. */
  onDismiss: () => void
}

export function AegisSuggestionsBanner({
  suggestions,
  blocked,
  working,
  onPreview,
  onApply,
  onDismiss,
}: AegisSuggestionsBannerProps) {
  const { t } = useTranslation("student")

  // Tray expansion: collapsed by default so the banner stays a
  // one-line pill. Reset to collapsed every time the suggestions
  // change identity (e.g. analyzer fired again on edited input);
  // a stale preview from a different draft would be misleading.
  const [expanded, setExpanded] = useState(false)
  const [selected, setSelected] = useState<Set<number>>(
    () => new Set(suggestions.map((_, i) => i)),
  )
  const [preview, setPreview] = useState<string | null>(null)

  // When the suggestions array identity changes, re-init the
  // selection (default all checked) and drop any preview. We use
  // a stable JSON key over kind+text so flipping selections inside
  // the same suggestion set doesn't fire this. (Suggestion arrays
  // come from the cached live verdict; identity changes only when
  // the analyzer ships a new verdict.)
  const sigKey = useMemo(
    () => JSON.stringify(suggestions.map((s) => [s.kind, s.text])),
    [suggestions],
  )
  useEffect(() => {
    setSelected(new Set(suggestions.map((_, i) => i)))
    setPreview(null)
    setExpanded(false)
    // Re-run only on the structural signature, not on parent re-renders.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [sigKey])

  const compactState: "idle" | "blocked" | "working" = working
    ? "working"
    : blocked
      ? "blocked"
      : "idle"

  const containerClass = cn(
    "rounded-md border",
    compactState === "blocked"
      ? "border-rose-300 bg-rose-50 dark:bg-rose-950/40 dark:border-rose-800"
      : "border-amber-300 bg-amber-50 dark:bg-amber-950/40 dark:border-amber-800",
  )

  const toggle = (i: number) => {
    setSelected((prev) => {
      const next = new Set(prev)
      if (next.has(i)) next.delete(i)
      else next.add(i)
      return next
    })
    // Editing the selection invalidates any in-progress preview.
    setPreview(null)
  }

  const handlePreviewClick = async () => {
    if (working || selected.size === 0) return
    const chosen = suggestions.filter((_, i) => selected.has(i))
    const rewritten = await onPreview(chosen)
    setPreview(rewritten ?? null)
  }

  const handleApplyClick = () => {
    if (!preview) return
    onApply(preview)
    // Parent will hide / replace the banner naturally on the
    // next render; collapse here so the tray doesn't flash open
    // for the rewritten input's own (likely empty) verdict.
    setExpanded(false)
    setPreview(null)
  }

  return (
    <div className={containerClass} role="status" aria-live="polite">
      {/* Compact row: status + count + Review/Collapse + dismiss. */}
      <div className="flex items-center gap-2 px-3 py-2">
        <AegisShieldFilled
          size={20}
          className="rounded shrink-0"
          aria-hidden="true"
        />
        <div className="flex-1 min-w-0">
          <div className="text-sm font-medium leading-tight">
            {t("aegis.banner.title", { count: suggestions.length })}
          </div>
          <div className="text-xs text-muted-foreground leading-tight">
            {compactState === "blocked"
              ? t("aegis.banner.blockedHint")
              : t("aegis.banner.idleHintReview")}
          </div>
        </div>
        <Button
          type="button"
          size="sm"
          variant={expanded ? "outline" : "default"}
          onClick={() => setExpanded((v) => !v)}
          className="shrink-0 gap-1.5"
          aria-expanded={expanded}
        >
          <ChevronDown
            aria-hidden="true"
            className={cn(
              "w-3.5 h-3.5 transition-transform",
              expanded && "rotate-180",
            )}
          />
          {expanded ? t("aegis.banner.collapse") : t("aegis.banner.review")}
        </Button>
        <Button
          type="button"
          size="sm"
          variant="ghost"
          onClick={onDismiss}
          aria-label={t("aegis.banner.dismiss")}
          title={t("aegis.banner.dismiss")}
          className="h-7 w-7 p-0 shrink-0"
        >
          <X className="h-4 w-4" />
        </Button>
      </div>

      {/* Expanded tray: checkbox list + preview/apply controls. */}
      {expanded && (
        <div className="border-t border-amber-200/70 dark:border-amber-800/70 px-3 py-3 space-y-3">
          <p className="text-xs text-muted-foreground">
            {t("aegis.banner.trayInstruction")}
          </p>
          <ul className="space-y-2">
            {suggestions.map((s, i) => {
              const kindLabel = t(`aegis.kinds.${s.kind}`, {
                defaultValue: s.kind,
              })
              const id = `aegis-suggestion-${i}`
              return (
                <li
                  key={`${i}-${s.kind}`}
                  className="flex items-start gap-2 rounded border bg-background/60 dark:bg-background/30 p-2"
                >
                  <Checkbox
                    id={id}
                    checked={selected.has(i)}
                    onCheckedChange={() => toggle(i)}
                    className="mt-0.5"
                    disabled={working}
                  />
                  <label
                    htmlFor={id}
                    className="flex-1 min-w-0 cursor-pointer space-y-1"
                  >
                    <Badge
                      variant="secondary"
                      className="text-[10px] uppercase tracking-wide"
                    >
                      {kindLabel}
                    </Badge>
                    <p className="text-sm leading-snug">{s.text}</p>
                    {s.explanation && (
                      <p className="text-xs leading-relaxed text-muted-foreground">
                        {s.explanation}
                      </p>
                    )}
                  </label>
                </li>
              )
            })}
          </ul>

          {preview ? (
            <div className="rounded border bg-background p-2 space-y-2">
              <div className="text-[10px] font-semibold tracking-widest uppercase text-muted-foreground">
                {t("aegis.banner.previewHeader")}
              </div>
              <p className="text-sm leading-snug whitespace-pre-wrap">
                {preview}
              </p>
              <div className="flex flex-wrap gap-2 pt-1">
                <Button
                  type="button"
                  size="sm"
                  variant="default"
                  onClick={handleApplyClick}
                  disabled={working}
                >
                  {t("aegis.banner.applyToDraft")}
                </Button>
                <Button
                  type="button"
                  size="sm"
                  variant="ghost"
                  onClick={() => setPreview(null)}
                  disabled={working}
                >
                  {t("aegis.banner.previewDiscard")}
                </Button>
              </div>
            </div>
          ) : (
            <div className="flex flex-wrap gap-2">
              <Button
                type="button"
                size="sm"
                variant="default"
                onClick={handlePreviewClick}
                disabled={working || selected.size === 0}
                className="gap-1.5"
              >
                <Wand2 className="w-3.5 h-3.5" />
                {working
                  ? t("aegis.banner.working")
                  : t("aegis.banner.preview")}
              </Button>
              <Button
                type="button"
                size="sm"
                variant="ghost"
                onClick={() => setExpanded(false)}
                disabled={working}
              >
                {t("aegis.banner.cancel")}
              </Button>
            </div>
          )}
        </div>
      )}
    </div>
  )
}
