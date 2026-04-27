/**
 * Right-rail Feedback panel for the Aegis prompt-coaching feature.
 *
 * Renders three sections in vertical order, mirroring the figma mockup:
 *
 *   1. **Quality banner** -- one of high / medium / low, colour-coded.
 *      Driven by `analysis.overall_score` (0..=10) using a fixed
 *      threshold split. The "Low" banner reuses the same red shell
 *      as the figma's "Test why it's bad" empty-prompt example so we
 *      don't need separate styling for short or low-effort prompts.
 *
 *   2. **Prompt Analysis** -- three short callouts (structural
 *      clarity, terminology specificity, missing constraint). Each
 *      pulls its label + one-sentence rationale from the analysis
 *      row directly; the panel doesn't reformat or summarise.
 *
 *   3. **History** -- every prior user-turn analysis on this
 *      conversation, newest first, with a date-coarsening header
 *      ("Today" / "Yesterday" / explicit date) and the overall
 *      score formatted as `N.M` (always one-decimal so 9 reads as
 *      `9.0`, matching the figma style).
 *
 * The panel is a pure view -- no fetching of its own. The chat
 * page hands it `analyses` (from the conversation detail query)
 * and `latest` (the in-flight analysis received over SSE during
 * the current send, which lives outside the React Query cache
 * until the conversation detail refetches). When `latest` is
 * present we render IT as the primary panel content even if it
 * isn't yet in `analyses`, which keeps the panel from blanking
 * during the brief window between SSE arrival and refetch.
 */
import { useTranslation } from "react-i18next"
import { Badge } from "@/components/ui/badge"
import { cn } from "@/lib/utils"
import type { PromptAnalysis } from "@/lib/types"

interface AegisFeedbackPanelProps {
  /** All analyses fetched with the conversation detail. Oldest first. */
  analyses: PromptAnalysis[]
  /**
   * Optional in-flight analysis received over SSE while the conversation
   * detail query hasn't yet refetched. Takes precedence over `analyses`
   * for the "latest" slot so the panel never flashes blank between SSE
   * arrival and the refetch settling.
   */
  latest: PromptAnalysis | null
  /**
   * True while the user's message is in-flight and we expect an
   * analysis to arrive shortly. Drives the "Scoring your prompt…"
   * placeholder so the panel doesn't look broken during the round-trip.
   */
  pending: boolean
}

const HIGH_THRESHOLD = 8
const MEDIUM_THRESHOLD = 5

type Quality = "high" | "medium" | "low"

function classifyQuality(score: number): Quality {
  if (score >= HIGH_THRESHOLD) return "high"
  if (score >= MEDIUM_THRESHOLD) return "medium"
  return "low"
}

/** Format `8` as `8.0`, `9.2` as `9.2`. Matches the figma's history column. */
function formatScore(score: number): string {
  return score.toFixed(1)
}

/**
 * Coarsen `iso` to one of "Today" / "Yesterday" / a localised date.
 * Used as the header for groups of historical analyses; the figma's
 * "3:42 PM TODAY" / "YESTERDAY" / "MARCH 23" pattern.
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
}: AegisFeedbackPanelProps) {
  const { t, i18n } = useTranslation("student")

  // Latest analysis = explicit `latest` (live SSE event) > newest
  // entry from `analyses` (conversation detail). The merge order
  // matters: a freshly arrived SSE row should win over the cached
  // detail's last entry until the next refetch settles.
  const fallbackLatest = analyses.length > 0 ? analyses[analyses.length - 1] : null
  const current = latest ?? fallbackLatest

  // History section: every entry except the one we're showing as
  // "current". Newest first to match the figma's reverse-chrono
  // history list.
  const historyEntries: PromptAnalysis[] = [...analyses]
    .filter((a) => a.message_id !== current?.message_id)
    .reverse()

  return (
    <div className="flex flex-col h-full overflow-y-auto pl-4 gap-4">
      <div className="flex items-center justify-between">
        <h2 className="text-lg font-semibold">
          {t("aegis.panelTitle")}
        </h2>
        <Badge variant="outline" className="text-xs">
          {t("aegis.modeBadge")}
        </Badge>
      </div>

      <QualityCard analysis={current} pending={pending} />

      {current && <PromptAnalysisSection analysis={current} />}

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

function QualityCard({
  analysis,
  pending,
}: {
  analysis: PromptAnalysis | null
  pending: boolean
}) {
  const { t } = useTranslation("student")

  if (!analysis) {
    if (pending) {
      return (
        <div className="rounded-md border border-dashed p-4 space-y-1">
          <div className="text-sm font-medium text-muted-foreground">
            {t("aegis.pendingTitle")}
          </div>
          <p className="text-xs text-muted-foreground">
            {t("aegis.pendingBody")}
          </p>
        </div>
      )
    }
    return (
      <div className="rounded-md border border-dashed p-4 space-y-1">
        <div className="text-sm font-medium">{t("aegis.emptyTitle")}</div>
        <p className="text-xs text-muted-foreground">{t("aegis.emptyBody")}</p>
      </div>
    )
  }

  const quality = classifyQuality(analysis.overall_score)
  // Quality classes match the mockup's coloured banners: green
  // (high), amber (medium), red (low). Tailwind's emerald/amber/red
  // 100/300 + 800 family gives roughly the figma's saturation
  // without inventing custom palette tokens.
  const qualityClass = cn(
    "rounded-md border p-4 space-y-1",
    quality === "high" &&
      "border-emerald-300 bg-emerald-50 dark:bg-emerald-950/40 dark:border-emerald-800",
    quality === "medium" &&
      "border-amber-300 bg-amber-50 dark:bg-amber-950/40 dark:border-amber-800",
    quality === "low" &&
      "border-rose-300 bg-rose-50 dark:bg-rose-950/40 dark:border-rose-800",
  )
  const titleClass = cn(
    "text-sm font-semibold",
    quality === "high" && "text-emerald-900 dark:text-emerald-200",
    quality === "medium" && "text-amber-900 dark:text-amber-200",
    quality === "low" && "text-rose-900 dark:text-rose-200",
  )
  const titleKey =
    quality === "high"
      ? "aegis.qualityHigh"
      : quality === "medium"
        ? "aegis.qualityMedium"
        : "aegis.qualityLow"

  return (
    <div className={qualityClass} data-testid="aegis-quality-card">
      <div className="flex items-center justify-between">
        <div className={titleClass}>{t(titleKey)}</div>
        <div className={cn("text-sm font-bold tabular-nums", titleClass)}>
          {formatScore(analysis.overall_score)}
        </div>
      </div>
      {analysis.missing_constraint_feedback && (
        <p className="text-xs leading-snug text-foreground/90">
          {analysis.missing_constraint_feedback}
        </p>
      )}
    </div>
  )
}

function PromptAnalysisSection({ analysis }: { analysis: PromptAnalysis }) {
  const { t } = useTranslation("student")

  // Resolve the localised label for each callout, falling back to
  // the raw analyzer string if the model coined a label outside
  // the documented enum -- preferable to rendering an empty
  // `_LABEL_` translation token in the middle of the heading.
  const structuralLabel =
    t(`aegis.labels.structural_clarity.${analysis.structural_clarity_label}`, {
      defaultValue: analysis.structural_clarity_label,
    })
  const terminologyLabel = t(
    `aegis.labels.terminology.${analysis.terminology_label}`,
    { defaultValue: analysis.terminology_label },
  )
  const missingConstraintLabel = t(
    `aegis.labels.missing_constraint.${analysis.missing_constraint_label}`,
    { defaultValue: analysis.missing_constraint_label },
  )

  return (
    <section className="space-y-3">
      <div className="text-[10px] font-semibold tracking-widest text-muted-foreground uppercase">
        {t("aegis.analysisHeader")}
      </div>
      <Callout
        heading={t("aegis.structuralClarityHeading", {
          label: structuralLabel,
        })}
        body={analysis.structural_clarity_feedback}
      />
      <Callout
        heading={`${t("aegis.terminologyHeading")}${
          terminologyLabel ? ` -${terminologyLabel}` : ""
        }`}
        body={analysis.terminology_feedback}
      />
      <Callout
        heading={`${t("aegis.missingConstraintHeading")}${
          missingConstraintLabel ? ` -${missingConstraintLabel}` : ""
        }`}
        body={analysis.missing_constraint_feedback}
      />
    </section>
  )
}

function Callout({ heading, body }: { heading: string; body: string }) {
  return (
    <div className="space-y-1">
      <div className="text-sm font-medium">{heading}</div>
      {body && (
        <p className="text-xs leading-snug text-muted-foreground">{body}</p>
      )}
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

  // Group consecutive entries with the same date label into one
  // header. Re-uses dateGroupLabel for both the comparison key and
  // the visible string so they can never drift.
  const groups: { label: string; items: PromptAnalysis[] }[] = []
  for (const entry of entries) {
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
  const quality = classifyQuality(analysis.overall_score)
  const scoreClass = cn(
    "text-xs font-bold tabular-nums px-2 py-1 rounded",
    quality === "high" &&
      "bg-emerald-100 text-emerald-900 dark:bg-emerald-900/40 dark:text-emerald-200",
    quality === "medium" &&
      "bg-amber-100 text-amber-900 dark:bg-amber-900/40 dark:text-amber-200",
    quality === "low" &&
      "bg-rose-100 text-rose-900 dark:bg-rose-900/40 dark:text-rose-200",
  )
  // Headline = whichever feedback string the analyzer produced
  // first that is non-empty. The figma's history rows show a
  // short snippet of the prompt itself, but we don't ship the
  // user-message text into the analysis row (it's already on
  // the messages table); the missing-constraint feedback is
  // typically the most actionable one-liner so it doubles as a
  // recap for the row.
  const recap =
    analysis.missing_constraint_feedback ||
    analysis.structural_clarity_feedback ||
    analysis.terminology_feedback
  return (
    <div className="flex items-start justify-between gap-2 rounded border p-2">
      <p className="text-xs text-muted-foreground line-clamp-2 flex-1">
        {recap || "-"}
      </p>
      <span className={scoreClass}>{formatScore(analysis.overall_score)}</span>
    </div>
  )
}
