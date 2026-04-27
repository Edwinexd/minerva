/**
 * Right-rail Feedback panel for the Aegis prompt-coaching feature.
 *
 * Shows actionable SUGGESTIONS for the student's current draft; not
 * scores, not a rubric. Per the project brief and Herodotou et al.
 * (2025), grading a student's prompt risks exactly the condescending
 * tone we are trying to avoid; we surface concrete improvements
 * instead and let the student decide what (if anything) to act on.
 *
 * Three sections in vertical order:
 *
 *   1. **Suggestions for the current draft**; 0..=3 short tagged
 *      sentences ("you could say what you've already tried"). Empty
 *      = a small "looks good" affirmation rather than a blank slot.
 *
 *   2. **Mode toggle**; Beginner / Expert badge. Drives the
 *      analyzer's calibration (separate hook; `useAegisMode`); the
 *      panel just renders the toggle UI.
 *
 *   3. **History**; past prompts in this conversation that had
 *      suggestions, newest first. Collapsed to one-liner each.
 *
 * Pure view; no fetching of its own. Receives `analyses` (history,
 * from conversation detail) and `latest` (the live verdict for the
 * draft the student is currently composing) from the parent chat
 * page; the parent owns the analyzer call.
 */
import { X } from "lucide-react"
import { useTranslation } from "react-i18next"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { AegisShieldFilled } from "@/components/icons/aegis-shield-filled"
import { cn } from "@/lib/utils"
import type { AegisSuggestion, PromptAnalysis } from "@/lib/types"
import { useAegisMode } from "./use-aegis-mode"

interface AegisFeedbackPanelProps {
  /** All persisted analyses for this conversation. Oldest first. */
  analyses: PromptAnalysis[]
  /**
   * Live verdict for the student's CURRENT draft (the prompt they
   * are typing right now, not yet sent). Drives the Suggestions
   * section. Null when the analyzer hasn't fired yet OR the input
   * is below the live-analyzer's MIN_LENGTH OR aegis is off.
   */
  latest: PromptAnalysis | null
  /**
   * True while the live analyzer's request is in flight. Renders
   * the "thinking..." placeholder so the panel doesn't look frozen
   * during the round-trip.
   */
  pending: boolean
  /**
   * Called when the student dismisses the panel via the header X.
   * Caller persists the choice (typically via `useAegisPanelVisible`)
   * and stops rendering this component until the student
   * un-dismisses it via the chat-side "Aegis" affordance.
   */
  onHide: () => void
}

/**
 * Coarsen `iso` to one of "Today" / "Yesterday" / a localised date.
 * Used as the header for groups of historical analyses.
 */
function dateGroupLabel(
  iso: string,
  todayLabel: string,
  yesterdayLabel: string,
  locale: string,
): string {
  const d = new Date(iso)
  const now = new Date()
  const sameDay = (a: Date, b: Date) =>
    a.getFullYear() === b.getFullYear() &&
    a.getMonth() === b.getMonth() &&
    a.getDate() === b.getDate()

  if (sameDay(d, now)) return todayLabel
  const yesterday = new Date(now)
  yesterday.setDate(now.getDate() - 1)
  if (sameDay(d, yesterday)) return yesterdayLabel

  return d.toLocaleDateString(locale, { month: "long", day: "numeric" })
}

export function AegisFeedbackPanel({
  analyses,
  latest,
  pending,
  onHide,
}: AegisFeedbackPanelProps) {
  const { t, i18n } = useTranslation("student")
  const [mode, setMode] = useAegisMode()
  const toggleMode = () =>
    setMode(mode === "beginner" ? "expert" : "beginner")

  // What we render in the "current" slot. Live verdict wins; if the
  // student isn't currently typing (no live verdict, no pending
  // call) we fall back to the most recent persisted entry so the
  // panel isn't blank between turns.
  const fallbackLatest = analyses.length > 0 ? analyses[analyses.length - 1] : null
  const current = latest ?? fallbackLatest

  // History = every persisted analysis except the one we're showing
  // as "current". Reversed for newest-first display. Persisted rows
  // with empty suggestion lists are still kept (a "looks good" turn
  // is still useful context for the student).
  const historyEntries: PromptAnalysis[] = [...analyses]
    .filter((a) => a.message_id !== current?.message_id)
    .reverse()

  return (
    <div className="flex flex-col h-full overflow-y-auto pl-4 gap-4">
      <div className="flex items-center justify-between gap-2">
        <h2 className="text-lg font-semibold flex items-center gap-2 min-w-0">
          <AegisShieldFilled size={20} className="shrink-0 rounded-md" />
          <span className="truncate">{t("aegis.panelTitle")}</span>
        </h2>
        <div className="flex items-center gap-1 shrink-0">
          {/*
            Mode toggle. Renders as a Badge inside a button so the
            visual matches the figma pill while the click target is
            a real semantic button (keyboard-accessible, announced
            as a toggle). The toggle's effect lands server-side:
            each Beginner/Expert flip changes the rubric the
            analyzer runs under for the next analyze call.
          */}
          <button
            type="button"
            onClick={toggleMode}
            className="rounded-4xl focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring/50"
            aria-pressed={mode === "expert"}
            title={t("aegis.modeToggleHint")}
          >
            <Badge
              variant={mode === "expert" ? "default" : "outline"}
              className="text-xs cursor-pointer"
            >
              {mode === "expert"
                ? t("aegis.modeExpert")
                : t("aegis.modeBeginner")}
            </Badge>
          </button>
          <Button
            type="button"
            variant="ghost"
            size="sm"
            onClick={onHide}
            aria-label={t("aegis.hidePanel")}
            title={t("aegis.hidePanel")}
            className="h-7 w-7 p-0"
          >
            <X className="h-4 w-4" />
          </Button>
        </div>
      </div>

      <CurrentSection analysis={current} pending={pending} />

      {historyEntries.length > 0 && (
        <HistorySection
          entries={historyEntries}
          locale={i18n.language}
          todayLabel={t("aegis.historyToday")}
          yesterdayLabel={t("aegis.historyYesterday")}
        />
      )}
    </div>
  )
}

/**
 * Renders the "for your current draft" slot. Three exclusive states:
 *
 *   * No analysis at all (student hasn't typed enough yet, or aegis
 *     just turned on) -> empty-state copy explaining what the panel
 *     does without claiming anything specific.
 *   * Pending (live request in flight) -> a "thinking..." placeholder.
 *   * Have an analysis -> either the suggestion list, or the
 *     "looks good" affirmation when there are no suggestions.
 */
function CurrentSection({
  analysis,
  pending,
}: {
  analysis: PromptAnalysis | null
  pending: boolean
}) {
  const { t } = useTranslation("student")

  if (!analysis) {
    if (pending) {
      return <PlaceholderCard kind="pending" />
    }
    return <PlaceholderCard kind="empty" />
  }

  if (analysis.suggestions.length === 0) {
    return (
      <div className="rounded-md border border-emerald-300 bg-emerald-50 dark:bg-emerald-950/40 dark:border-emerald-800 p-4 space-y-1">
        <div className="text-sm font-semibold text-emerald-900 dark:text-emerald-200">
          {t("aegis.looksGoodTitle")}
        </div>
        <p className="text-xs text-foreground/80">
          {t("aegis.looksGoodBody")}
        </p>
      </div>
    )
  }

  return (
    <section className="space-y-2">
      <div className="text-[10px] font-semibold tracking-widest text-muted-foreground uppercase">
        {t("aegis.suggestionsHeader")}
      </div>
      {analysis.suggestions.map((s, i) => (
        <SuggestionRow key={i} suggestion={s} />
      ))}
    </section>
  )
}

function PlaceholderCard({ kind }: { kind: "pending" | "empty" }) {
  const { t } = useTranslation("student")
  const titleKey = kind === "pending" ? "aegis.pendingTitle" : "aegis.emptyTitle"
  const bodyKey = kind === "pending" ? "aegis.pendingBody" : "aegis.emptyBody"
  return (
    <div className="rounded-md border border-dashed p-4 space-y-1">
      <div
        className={cn(
          "text-sm font-medium",
          kind === "pending" && "text-muted-foreground",
        )}
      >
        {t(titleKey)}
      </div>
      <p className="text-xs text-muted-foreground">{t(bodyKey)}</p>
    </div>
  )
}

/**
 * One suggestion. The kind tag is rendered as a small badge; it
 * gives the student a sense of WHAT category of improvement this
 * is (clarity, rationale, etc.) before they read the body. The
 * card itself is tinted by severity:
 *   * `high`   -> rose  (must fix to get a useful answer)
 *   * `medium` -> amber (would meaningfully sharpen the answer)
 *   * `low`    -> sky   (polish; nice-to-have)
 * Unknown severities (legacy rows, unrecognised values) render as
 * a neutral border so the row still parses visually.
 */
export function SuggestionRow({
  suggestion,
}: {
  suggestion: AegisSuggestion
}) {
  const { t } = useTranslation("student")
  // Localise the kind tag if we know it, else show the raw string.
  const kindLabel = t(`aegis.kinds.${suggestion.kind}`, {
    defaultValue: suggestion.kind,
  })
  const severity = suggestion.severity as "high" | "medium" | "low" | string
  const cardClass = cn(
    "rounded border p-3 space-y-2",
    severity === "high" &&
      "border-rose-300 bg-rose-50/60 dark:bg-rose-950/30 dark:border-rose-800",
    severity === "medium" &&
      "border-amber-300 bg-amber-50/60 dark:bg-amber-950/30 dark:border-amber-800",
    severity === "low" &&
      "border-sky-300 bg-sky-50/60 dark:bg-sky-950/30 dark:border-sky-800",
  )
  // Same palette logic for the kind badge so the eye groups the
  // tag with its card.
  const kindBadgeClass = cn(
    "text-[10px] uppercase tracking-wide",
    severity === "high" &&
      "bg-rose-100 text-rose-900 dark:bg-rose-900/50 dark:text-rose-100 border-rose-300/40",
    severity === "medium" &&
      "bg-amber-100 text-amber-900 dark:bg-amber-900/50 dark:text-amber-100 border-amber-300/40",
    severity === "low" &&
      "bg-sky-100 text-sky-900 dark:bg-sky-900/50 dark:text-sky-100 border-sky-300/40",
  )
  return (
    <div className={cardClass}>
      <Badge variant="secondary" className={kindBadgeClass}>
        {kindLabel}
      </Badge>
      <p className="text-sm leading-snug">{suggestion.text}</p>
    </div>
  )
}

function HistorySection({
  entries,
  locale,
  todayLabel,
  yesterdayLabel,
}: {
  entries: PromptAnalysis[]
  locale: string
  todayLabel: string
  yesterdayLabel: string
}) {
  const { t } = useTranslation("student")

  // Group consecutive entries with the same date label.
  const groups: { label: string; items: PromptAnalysis[] }[] = []
  for (const entry of entries) {
    if (!entry.created_at) continue // live entries (shouldn't appear in history)
    const label = dateGroupLabel(
      entry.created_at,
      todayLabel,
      yesterdayLabel,
      locale,
    )
    const last = groups[groups.length - 1]
    if (last && last.label === label) {
      last.items.push(entry)
    } else {
      groups.push({ label, items: [entry] })
    }
  }

  return (
    <section className="space-y-3 pt-2">
      <div className="text-[10px] font-semibold tracking-widest text-muted-foreground uppercase">
        {t("aegis.historyHeader")}
      </div>
      {groups.map((group) => (
        <div key={group.label} className="space-y-2">
          <div className="text-[10px] font-semibold tracking-wider text-muted-foreground uppercase">
            {group.label}
          </div>
          {group.items.map((a) => (
            <HistoryRow key={a.id} analysis={a} />
          ))}
        </div>
      ))}
    </section>
  )
}

function HistoryRow({ analysis }: { analysis: PromptAnalysis }) {
  const { t } = useTranslation("student")
  // Headline: top suggestion's text, or a "looks good" pill when
  // the persisted row had no suggestions. Either way the row stays
  // a single line so a long history doesn't dominate the rail.
  const headline =
    analysis.suggestions[0]?.text ?? t("aegis.historyLooksGood")
  const tag =
    analysis.suggestions[0]?.kind ?? "ok"
  return (
    <div className="flex items-start gap-2 rounded border p-2">
      <Badge
        variant={analysis.suggestions.length > 0 ? "secondary" : "outline"}
        className="text-[9px] uppercase tracking-wide shrink-0"
      >
        {analysis.suggestions.length > 0
          ? t(`aegis.kinds.${tag}`, { defaultValue: tag })
          : t("aegis.historyLooksGoodTag")}
      </Badge>
      <p className="text-xs text-muted-foreground line-clamp-2 flex-1">
        {headline}
      </p>
    </div>
  )
}
