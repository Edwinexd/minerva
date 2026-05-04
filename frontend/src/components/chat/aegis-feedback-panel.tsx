/**
 * Right-rail History panel for the Aegis prompt-coaching feature.
 *
 * Pilot feedback was unanimous that the live "current draft"
 * suggestions belong above the input, not in a side rail; the
 * banner (`aegis-suggestions-banner.tsx`) now owns that role and
 * carries the full review/preview/apply flow. The side panel kept
 * showing the same suggestions in parallel, which (a) duplicated
 * the banner, (b) competed with it for the student's attention,
 * and (c) made every "looks good" state appear twice. We removed
 * the duplicate and kept the panel for one job: a chronological
 * record of the analyses that fired earlier in this conversation.
 *
 * Two sections in vertical order:
 *
 *   1. **Header**; shield icon + "History" title + Beginner /
 *      Expert mode toggle (still drives the analyzer's calibration
 *      via `useAegisMode`) + dismiss X.
 *
 *   2. **History**; every persisted analysis for the conversation,
 *      newest first, grouped by date ("Today" / "Yesterday" /
 *      localised date). One-line per row. An empty state appears
 *      until the first analysis lands so the panel doesn't render
 *      a blank rail in a fresh conversation.
 *
 * Pure view; no fetching of its own. Receives `analyses` (history,
 * from conversation detail) from the parent chat page.
 */
import { ChevronDown, X } from "lucide-react"
import { useState } from "react"
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
  onHide,
}: AegisFeedbackPanelProps) {
  const { t, i18n } = useTranslation("student")
  const [mode, setMode] = useAegisMode()
  const toggleMode = () =>
    setMode(mode === "beginner" ? "expert" : "beginner")

  // Newest first. We keep persisted rows with empty suggestion
  // lists ("looks good" turns) ; they're a useful signal in the
  // log that a draft was sent without the analyzer flagging
  // anything, and removing them would create gaps in the timeline.
  const historyEntries: PromptAnalysis[] = [...analyses].reverse()

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

      {historyEntries.length === 0 ? (
        <EmptyHistoryCard />
      ) : (
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
 * Placeholder for a brand-new conversation with no analyses yet.
 * Kept low-key (dashed border, muted text) so it doesn't compete
 * with the chat content; just enough to explain why the rail
 * isn't blank by mistake.
 */
function EmptyHistoryCard() {
  const { t } = useTranslation("student")
  return (
    <div className="rounded-md border border-dashed p-4 space-y-1">
      <div className="text-sm font-medium">{t("aegis.historyEmptyTitle")}</div>
      <p className="text-xs text-muted-foreground">
        {t("aegis.historyEmptyBody")}
      </p>
    </div>
  )
}

/**
 * One suggestion. Expandable so the student can read the longer
 * `explanation` paragraph; default is collapsed for low noise.
 *
 * The kind tag is rendered as a small badge; it gives the student
 * a sense of WHAT category of improvement this is (clarity,
 * rationale, etc.) before they read the body. The card itself is
 * tinted by severity:
 *   * `high`   -> rose  (must fix to get a useful answer)
 *   * `medium` -> amber (would meaningfully sharpen the answer)
 *   * `low`    -> sky   (polish; nice-to-have)
 * Unknown severities (legacy rows, unrecognised values) render as
 * a neutral border so the row still parses visually.
 *
 * Exported so the banner's review tray can reuse the same card
 * shape ; kept here so the visual stays the single source of
 * truth for "this is what an Aegis suggestion looks like".
 */
export function SuggestionRow({
  suggestion,
}: {
  suggestion: AegisSuggestion
}) {
  const { t } = useTranslation("student")
  const [expanded, setExpanded] = useState(false)
  const explanation = suggestion.explanation?.trim() ?? ""
  const expandable = explanation.length > 0
  // Localise the kind tag if we know it, else show the raw string.
  const kindLabel = t(`aegis.kinds.${suggestion.kind}`, {
    defaultValue: suggestion.kind,
  })
  const severity = suggestion.severity as "high" | "medium" | "low" | string
  const cardClass = cn(
    "w-full text-left rounded border p-3 space-y-2 transition-colors",
    severity === "high" &&
      "border-rose-300 bg-rose-50/60 dark:bg-rose-950/30 dark:border-rose-800",
    severity === "medium" &&
      "border-amber-300 bg-amber-50/60 dark:bg-amber-950/30 dark:border-amber-800",
    severity === "low" &&
      "border-sky-300 bg-sky-50/60 dark:bg-sky-950/30 dark:border-sky-800",
    expandable &&
      "cursor-pointer hover:brightness-[0.98] dark:hover:brightness-110 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring/50",
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

  // Static (no explanation) -> render as a div; no click target,
  // no chevron, no aria-expanded. Keeps the row honest about what
  // it actually offers when the analyzer didn't ship the field.
  if (!expandable) {
    return (
      <div className={cardClass}>
        <Badge variant="secondary" className={kindBadgeClass}>
          {kindLabel}
        </Badge>
        <p className="text-sm leading-snug">{suggestion.text}</p>
      </div>
    )
  }

  return (
    <button
      type="button"
      onClick={() => setExpanded((v) => !v)}
      aria-expanded={expanded}
      className={cardClass}
      title={
        expanded
          ? t("aegis.suggestionCollapseHint")
          : t("aegis.suggestionExpandHint")
      }
    >
      <div className="flex items-start gap-2">
        <Badge variant="secondary" className={kindBadgeClass}>
          {kindLabel}
        </Badge>
        <ChevronDown
          aria-hidden="true"
          className={cn(
            "ml-auto h-4 w-4 shrink-0 text-muted-foreground transition-transform",
            expanded && "rotate-180",
          )}
        />
      </div>
      <p className="text-sm leading-snug">{suggestion.text}</p>
      {expanded && (
        <p className="text-xs leading-relaxed text-muted-foreground border-t pt-2 mt-1">
          {explanation}
        </p>
      )}
    </button>
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
