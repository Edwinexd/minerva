import { useId, useState } from "react"
import { useTranslation } from "react-i18next"
import { ChevronDown, ChevronRight } from "lucide-react"
import type {
  AssessmentCandidate,
  CaseAssessment,
  Coverage,
  ExternalCourse,
  FactMatch,
} from "@/lib/types"
import { TONE_CLASS, VERDICT_TONE, type Tone } from "./tones"

export function StatusBadge({ tone, children }: { tone: Tone; children: React.ReactNode }) {
  return (
    <span
      className={`inline-flex items-center whitespace-nowrap rounded-full border px-2 py-0.5 text-xs font-medium ${TONE_CLASS[tone]}`}
    >
      {children}
    </span>
  )
}

const COVERAGE_TONE: Record<Coverage, Tone> = { covered: "ok", partly: "warn", gap: "bad" }
const MATCH_TONE: Record<FactMatch, Tone> = { match: "ok", differs: "warn", unknown: "neutral" }

/** `7.5` and `4`, in the reader's locale. */
function useCredits() {
  const { i18n } = useTranslation()
  return (n: number | null | undefined) =>
    n == null ? null : new Intl.NumberFormat(i18n.language, { maximumFractionDigits: 1 }).format(n)
}

/**
 * A page citation: opens the document at the page in a new tab. The PDF
 * viewer reads `#page=N`; a text attachment simply opens.
 */
function Cite({ href, label, page }: { href: string | null; label: string; page: number | null }) {
  const { t } = useTranslation("student")
  if (!href || page == null) return null
  return (
    <a
      href={`${href}#page=${page}`}
      target="_blank"
      rel="noopener noreferrer"
      className="ml-1 inline-block whitespace-nowrap rounded bg-blue-50 px-1.5 text-[11px] font-medium text-blue-800 underline-offset-2 hover:underline dark:bg-blue-950/60 dark:text-blue-200"
    >
      {t("creditTransfer.assessment.cite", { doc: label, page })}
      <span className="sr-only"> ({t("creditTransfer.opensInNewTab")})</span>
    </a>
  )
}

/**
 * The structured answer of a credit transfer case: one card per candidate
 * DSV course (facts, outcome-by-outcome mapping, suggested verdict), weak
 * candidates collapsed, and the plans searched without a match.
 */
export function AssessmentView({ assessment }: { assessment: CaseAssessment }) {
  const { t } = useTranslation("student")
  const strong = assessment.candidates.filter((c) => c.verdict !== "none")
  const weak = assessment.candidates.filter((c) => c.verdict === "none")
  return (
    <div className="space-y-3 text-sm" data-assessment>
      <p className="rounded-md border border-dashed px-3 py-2 text-xs text-muted-foreground">
        {t("creditTransfer.assessment.advisory")}
      </p>
      {assessment.summary && <p>{assessment.summary}</p>}
      {assessment.external_courses.map((e) => (
        <ExternalFacts key={e.file} course={e} />
      ))}
      {assessment.candidates.length === 0 && (
        <p className="text-muted-foreground">{t("creditTransfer.assessment.noCandidates")}</p>
      )}
      {strong.map((c) => (
        <CandidateCard key={c.code} candidate={c} defaultOpen />
      ))}
      {weak.map((c) => (
        <CandidateCard key={c.code} candidate={c} defaultOpen={false} />
      ))}
      {assessment.ruled_out.length > 0 && <RuledOut items={assessment.ruled_out} />}
    </div>
  )
}

function ExternalFacts({ course }: { course: ExternalCourse }) {
  const { t } = useTranslation("student")
  const credits = useCredits()
  const name = [course.code, course.name].filter(Boolean).join(" ") || course.file
  const parts = [
    course.institution,
    course.credits != null
      ? t("creditTransfer.assessment.ects", { credits: credits(course.credits) })
      : t("creditTransfer.assessment.creditsUnknown"),
    [course.level, course.level_note].filter(Boolean).join(", "),
    course.outcome_count != null
      ? t("creditTransfer.assessment.outcomeCount", { count: course.outcome_count })
      : null,
    course.examination
      ? t("creditTransfer.assessment.examination", { examination: course.examination })
      : null,
  ].filter(Boolean)
  return (
    <p className="text-muted-foreground">
      <span className="font-semibold text-foreground">
        {t("creditTransfer.assessment.externalCourse")}:
      </span>{" "}
      {name}. {parts.join(". ")}.
      {course.url && (
        <>
          {" "}
          <a
            href={course.url}
            target="_blank"
            rel="noopener noreferrer"
            className="underline hover:text-foreground"
          >
            {course.file}
            <span className="sr-only"> ({t("creditTransfer.opensInNewTab")})</span>
          </a>
        </>
      )}
    </p>
  )
}

function CandidateCard({
  candidate: c,
  defaultOpen,
}: {
  candidate: AssessmentCandidate
  defaultOpen: boolean
}) {
  const { t } = useTranslation("student")
  const credits = useCredits()
  const [open, setOpen] = useState(defaultOpen)
  const headingId = useId()
  const bodyId = useId()
  const covered = c.outcomes.filter((o) => o.coverage === "covered").length
  const partly = c.outcomes.filter((o) => o.coverage === "partly").length
  const externalLabel = c.external_file.replace(/\.[^.]+$/, "")
  const verdictText =
    c.verdict === "full"
      ? t("creditTransfer.verdict.full")
      : c.verdict === "partial"
        ? t("creditTransfer.verdict.partial")
        : t("creditTransfer.verdict.none")

  return (
    <section
      aria-labelledby={headingId}
      className="rounded-lg border bg-background break-inside-avoid"
    >
      <div className="flex flex-wrap items-center justify-between gap-2 rounded-t-lg border-b bg-muted/40 px-4 py-3">
        {/* h2: the card follows the chat's h1 in the reading order. */}
        <h2 id={headingId} className="font-semibold">
          <a
            href={c.plan_url}
            target="_blank"
            rel="noopener noreferrer"
            className="hover:underline"
          >
            {c.code}: {c.name}
            <span className="sr-only"> ({t("creditTransfer.opensInNewTab")})</span>
          </a>{" "}
          <span className="text-xs font-normal text-muted-foreground">
            {t("creditTransfer.assessment.planVersion", { version: c.plan_version })}
          </span>
        </h2>
        <StatusBadge tone={VERDICT_TONE[c.verdict]}>{verdictText}</StatusBadge>
      </div>

      {!open && (
        <div className="flex flex-wrap items-center gap-2 px-4 py-2 text-muted-foreground">
          <span>{c.overlap_note || c.verdict_reason}</span>
          <button
            type="button"
            className="inline-flex items-center gap-1 underline hover:text-foreground print:hidden"
            aria-expanded={false}
            aria-controls={bodyId}
            onClick={() => setOpen(true)}
          >
            <ChevronRight className="h-3.5 w-3.5" aria-hidden="true" />
            {t("creditTransfer.assessment.showDetails")}
          </button>
        </div>
      )}

      <div id={bodyId} hidden={!open} className="space-y-3 px-4 py-3">
        {!defaultOpen && (
          <button
            type="button"
            className="inline-flex items-center gap-1 text-xs text-muted-foreground underline hover:text-foreground print:hidden"
            aria-expanded
            aria-controls={bodyId}
            onClick={() => setOpen(false)}
          >
            <ChevronDown className="h-3.5 w-3.5" aria-hidden="true" />
            {t("creditTransfer.assessment.hideDetails")}
          </button>
        )}
        <dl className="flex flex-wrap gap-x-6 gap-y-2">
          <Fact label={t("creditTransfer.assessment.credits")}>
            {t("creditTransfer.assessment.versus", {
              dsv: credits(c.dsv_credits) ?? "?",
              external: credits(c.external_credits) ?? "?",
            })}{" "}
            <StatusBadge tone={MATCH_TONE[c.credits_match]}>
              {t(`creditTransfer.match.${c.credits_match}`)}
            </StatusBadge>
          </Fact>
          <Fact label={t("creditTransfer.assessment.level")}>
            {t("creditTransfer.assessment.versus", {
              dsv: c.dsv_level ?? "?",
              external: c.external_level ?? "?",
            })}{" "}
            <StatusBadge tone={MATCH_TONE[c.level_match]}>
              {t(`creditTransfer.match.${c.level_match}`)}
            </StatusBadge>
          </Fact>
          <Fact label={t("creditTransfer.assessment.outcomesCovered")}>
            {t("creditTransfer.assessment.coveredOf", { covered, total: c.outcomes.length })}
            {partly > 0 && (
              <span className="font-normal text-muted-foreground">
                {" "}
                {t("creditTransfer.assessment.partlyCount", { count: partly })}
              </span>
            )}
          </Fact>
          {c.notes.map((n) => (
            <Fact key={n.label} label={n.label}>
              {n.value}
            </Fact>
          ))}
        </dl>

        {c.outcomes.length > 0 && (
          // A wide table scrolls sideways on a narrow screen, so the
          // region takes focus for keyboard users (see chat-surface.tsx).
          <section
            className="overflow-x-auto"
            // eslint-disable-next-line jsx-a11y/no-noninteractive-tabindex
            tabIndex={0}
            aria-label={t("creditTransfer.assessment.outcomeTable", { code: c.code })}
          >
            <table className="w-full border-collapse text-[13px]">
              <thead>
                <tr className="border-b text-left text-muted-foreground">
                  <th scope="col" className="w-[40%] py-2 pr-3 font-medium">
                    {t("creditTransfer.assessment.dsvOutcome", { code: c.code })}
                  </th>
                  <th scope="col" className="w-[40%] py-2 pr-3 font-medium">
                    {t("creditTransfer.assessment.externalOutcome", { course: externalLabel })}
                  </th>
                  <th scope="col" className="py-2 font-medium">
                    {t("creditTransfer.assessment.coverage")}
                  </th>
                </tr>
              </thead>
              <tbody>
                {c.outcomes.map((o, i) => (
                  <tr key={i} className="border-b align-top last:border-b-0">
                    <td className="py-2 pr-3">
                      {o.dsv_outcome}
                      <Cite href={c.plan_url} label={c.code} page={o.dsv_page} />
                    </td>
                    <td className="py-2 pr-3">
                      {o.external_outcome ?? (
                        <span className="text-muted-foreground">
                          {t("creditTransfer.assessment.noCorresponding")}
                        </span>
                      )}
                      {o.external_outcome && (
                        <Cite href={c.external_url} label={externalLabel} page={o.external_page} />
                      )}
                    </td>
                    <td className="py-2">
                      <StatusBadge tone={COVERAGE_TONE[o.coverage]}>
                        {t(`creditTransfer.coverage.${o.coverage}`)}
                      </StatusBadge>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </section>
        )}

        <div className={`rounded-md border px-3 py-2 ${TONE_CLASS[VERDICT_TONE[c.verdict]]}`}>
          <p className="font-semibold">
            {t("creditTransfer.assessment.suggestion")}: {c.verdict_title || verdictText}
          </p>
          {c.verdict_reason && <p className="mt-1 text-[13px]">{c.verdict_reason}</p>}
          {c.credited_modules.length > 0 && (
            <p className="mt-1 text-[13px]">
              {t("creditTransfer.assessment.creditedModules")}:{" "}
              {c.credited_modules
                .map((m) =>
                  t("creditTransfer.assessment.module", {
                    code: m.code,
                    name: m.name,
                    credits: credits(m.credits),
                  }),
                )
                .join(", ")}
            </p>
          )}
        </div>
      </div>
    </section>
  )
}

function Fact({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div>
      <dt className="text-xs text-muted-foreground">{label}</dt>
      <dd className="font-medium">{children}</dd>
    </div>
  )
}

function RuledOut({ items }: { items: CaseAssessment["ruled_out"] }) {
  const { t } = useTranslation("student")
  return (
    <details className="text-muted-foreground">
      <summary className="cursor-pointer">
        {t("creditTransfer.assessment.ruledOut", { count: items.length })}
      </summary>
      <ul className="mt-1 list-disc space-y-0.5 pl-6">
        {items.map((r) => (
          <li key={r.code}>
            <span className="font-medium text-foreground">
              {r.code} {r.name}
            </span>
            {r.reason && <>: {r.reason}</>}
          </li>
        ))}
      </ul>
    </details>
  )
}
