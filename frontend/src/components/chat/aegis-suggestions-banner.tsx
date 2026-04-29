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
import { Input } from "@/components/ui/input"
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select"
import { AegisShieldFilled } from "@/components/icons/aegis-shield-filled"
import { cn } from "@/lib/utils"
import type { AegisSuggestion } from "@/lib/types"

// Sentinel SelectItem `value` that means "the student wants to type
// a custom answer instead of picking from the dropdown". We store
// the actual answer in component state alongside; the Select's
// `value` prop only echoes this sentinel back so the trigger row
// labels itself "Other..." while the free-text input is on screen.
// Picked over an empty string because BaseUI's Select treats `""` as
// "no selection" and would visually clear the dropdown.
const CUSTOM_ANSWER_SENTINEL = "__aegis_custom__"

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
   * subset. Each suggestion is shipped with its `answer` field
   * already stamped from the dropdown selection (or the "Other..."
   * free-text input); the rewrite system prompt expects that field
   * present. Resolves to the rewritten draft text, or null on
   * failure (parent decides how to surface that; we just clear
   * any stale preview).
   */
  onPreview: (selectedWithAnswers: AegisSuggestion[]) => Promise<string | null>
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

  // Per-suggestion answer state. Two parallel maps keyed by the
  // suggestion's index in the current verdict:
  //   * `answers` ; the actual answer the student picked or typed,
  //     trimmed at preview time. Empty / missing means "no answer
  //     yet"; the Preview button stays disabled until every
  //     CHECKED suggestion has one.
  //   * `customMode` ; whether the student picked the "Other..."
  //     entry from the dropdown. When true the row swaps the
  //     dropdown's regular options for a free-text Input, and the
  //     dropdown trigger labels itself "Other...". We track this
  //     separately from `answers` because the same string could
  //     legitimately be one of the dropdown options OR a custom
  //     entry; the toggle disambiguates.
  const [answers, setAnswers] = useState<Record<number, string>>({})
  const [customMode, setCustomMode] = useState<Set<number>>(() => new Set())

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
    setAnswers({})
    setCustomMode(new Set())
    setPreview(null)
    // `expanded` is deliberately NOT reset here. If the student
    // already had the tray open and a new verdict lands (e.g. they
    // applied a rewrite, the input changed, and aegis returned a
    // fresh set of suggestions), keeping the tray expanded lets
    // them iterate without re-clicking Review every cycle. Fresh
    // banner mounts still start collapsed via useState's initial
    // value; this only protects an already-open tray from snapping
    // shut on every analyzer turn.
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

  // Dropdown change. `value` is either one of the suggestion's
  // options or the CUSTOM_ANSWER_SENTINEL. The latter flips the row
  // into custom-text mode and clears any prior canned answer so the
  // free-text input starts empty. Editing an answer also drops any
  // stale preview, same logic as toggling the checkbox.
  const setSelectAnswer = (i: number, value: string) => {
    if (value === CUSTOM_ANSWER_SENTINEL) {
      setCustomMode((prev) => {
        const next = new Set(prev)
        next.add(i)
        return next
      })
      setAnswers((prev) => ({ ...prev, [i]: "" }))
    } else {
      setCustomMode((prev) => {
        if (!prev.has(i)) return prev
        const next = new Set(prev)
        next.delete(i)
        return next
      })
      setAnswers((prev) => ({ ...prev, [i]: value }))
    }
    setPreview(null)
  }

  // Free-text input change while the row is in custom-text mode.
  // Just updates `answers[i]`; `customMode` stays true until the
  // student picks a regular option from the dropdown.
  const setCustomAnswer = (i: number, value: string) => {
    setAnswers((prev) => ({ ...prev, [i]: value }))
    setPreview(null)
  }

  // Preview is gated on EVERY checked suggestion having a non-empty
  // answer. Without this the rewrite call would land for some
  // suggestions with `answer: ""`, fall back to the system-prompt's
  // placeholder branch, and produce exactly the "specify what you
  // mean and explain..." filler the dropdowns are meant to replace.
  const allCheckedAnswered = useMemo(() => {
    for (const i of selected) {
      const a = answers[i]?.trim() ?? ""
      if (a.length === 0) return false
    }
    return true
  }, [selected, answers])

  const handlePreviewClick = async () => {
    if (working || selected.size === 0 || !allCheckedAnswered) return
    // Stamp each checked suggestion with its answer (trimmed) before
    // shipping. Order matches the original suggestion order so the
    // model sees context-stable indices if it cares. Empty trimmed
    // answers (shouldn't happen given the gate above; defensive)
    // ship the suggestion without an `answer` field so the rewrite
    // system prompt's placeholder branch fires for that one row
    // instead of stamping `answer: ""`.
    const chosen: AegisSuggestion[] = []
    suggestions.forEach((s, i) => {
      if (!selected.has(i)) return
      const a = (answers[i] ?? "").trim()
      chosen.push(a ? { ...s, answer: a } : s)
    })
    const rewritten = await onPreview(chosen)
    setPreview(rewritten ?? null)
  }

  const handleApplyClick = () => {
    if (!preview) return
    onApply(preview)
    // Drop the preview so the tray returns to the dropdown view
    // for the next cycle. We deliberately leave `expanded` alone:
    // the parent's onApply resets the analyzer cache so the banner
    // naturally hides until the next verdict lands, at which point
    // the still-expanded tray surfaces the new suggestions in the
    // same shape the student already had open.
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
              const opts = s.options ?? []
              const isChecked = selected.has(i)
              const isCustom = customMode.has(i)
              const currentAnswer = answers[i] ?? ""
              // Dropdown current value:
              //   * custom mode  ; the sentinel ("Other..." trigger label)
              //   * picked one   ; the option string itself
              //   * neither      ; undefined, so the placeholder shows
              const selectValue = isCustom
                ? CUSTOM_ANSWER_SENTINEL
                : opts.includes(currentAnswer)
                  ? currentAnswer
                  : undefined
              return (
                <li
                  key={`${i}-${s.kind}`}
                  className="flex items-start gap-2 rounded border bg-background/60 dark:bg-background/30 p-2"
                >
                  <Checkbox
                    id={id}
                    checked={isChecked}
                    onCheckedChange={() => toggle(i)}
                    className="mt-0.5"
                    disabled={working}
                  />
                  <div className="flex-1 min-w-0 space-y-2">
                    {/* Label + headline + explanation. The label
                        targets the checkbox; clicking the headline
                        text still toggles the suggestion. The
                        dropdown / input below sit OUTSIDE the
                        label so a click on them doesn't toggle
                        the checkbox. */}
                    <label
                      htmlFor={id}
                      className="block cursor-pointer space-y-1"
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
                    {/* Answer row. Disabled when the suggestion is
                        unchecked (the rewrite won't fold it in
                        anyway) or while a rewrite is in flight.
                        Hidden entirely if the suggestion has zero
                        options AND we're not in custom mode; the
                        free-text fallback handles legacy persisted
                        rows that pre-date the field. */}
                    <div className="space-y-2">
                      <div className="text-[10px] font-semibold uppercase tracking-wide text-muted-foreground">
                        {t("aegis.banner.answerLabel")}
                      </div>
                      {opts.length > 0 ? (
                        <Select
                          value={selectValue}
                          onValueChange={(v) =>
                            v && setSelectAnswer(i, v as string)
                          }
                          disabled={working || !isChecked}
                        >
                          <SelectTrigger className="w-full">
                            <SelectValue
                              placeholder={t(
                                "aegis.banner.answerPlaceholder",
                              )}
                            />
                          </SelectTrigger>
                          <SelectContent>
                            {opts.map((opt, oi) => (
                              <SelectItem key={oi} value={opt}>
                                {opt}
                              </SelectItem>
                            ))}
                            <SelectItem value={CUSTOM_ANSWER_SENTINEL}>
                              {t("aegis.banner.customAnswer")}
                            </SelectItem>
                          </SelectContent>
                        </Select>
                      ) : null}
                      {(isCustom || opts.length === 0) && (
                        <Input
                          value={currentAnswer}
                          onChange={(e) =>
                            setCustomAnswer(i, e.target.value)
                          }
                          placeholder={t(
                            "aegis.banner.customAnswerPlaceholder",
                          )}
                          disabled={working || !isChecked}
                          aria-label={t("aegis.banner.answerLabel")}
                        />
                      )}
                    </div>
                  </div>
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
            <div className="space-y-2">
              <div className="flex flex-wrap gap-2">
                <Button
                  type="button"
                  size="sm"
                  variant="default"
                  onClick={handlePreviewClick}
                  disabled={
                    working || selected.size === 0 || !allCheckedAnswered
                  }
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
              {!allCheckedAnswered && selected.size > 0 && (
                <p className="text-xs text-muted-foreground">
                  {t("aegis.banner.previewNeedsAnswers")}
                </p>
              )}
            </div>
          )}
        </div>
      )}
    </div>
  )
}
