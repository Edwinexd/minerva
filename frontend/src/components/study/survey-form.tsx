import { useMemo, useState } from "react"
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query"
import { useTranslation } from "react-i18next"
import { api } from "@/lib/api"
import { studySurveyQuery } from "@/lib/queries"
import type { StudySurveyAnswer, StudySurveyQuestion } from "@/lib/types"
import { Button } from "@/components/ui/button"
import { Card } from "@/components/ui/card"
import { Textarea } from "@/components/ui/textarea"
import { Skeleton } from "@/components/ui/skeleton"
import { useApiErrorMessage } from "@/lib/use-api-error"

/**
 * Renders a Likert + free-text survey, gated by `kind` (pre vs post).
 * Resumes from server-side `existing` answers so a tab close in the
 * middle doesn't lose progress. Submission validates client-side
 * (every question answered, Likert in range) BEFORE hitting the
 * server, but the server is the source of truth and re-validates.
 */
export function SurveyForm({
  courseId,
  kind,
}: {
  courseId: string
  kind: "pre" | "post"
}) {
  const { t } = useTranslation("study")
  const formatError = useApiErrorMessage()
  const queryClient = useQueryClient()
  const { data: survey, isLoading, error: loadError } = useQuery(
    studySurveyQuery(courseId, kind),
  )

  const [answers, setAnswers] = useState<Map<string, StudySurveyAnswer>>(
    new Map(),
  )
  const [validationError, setValidationError] = useState<string | null>(null)
  const [submitError, setSubmitError] = useState<unknown>(null)

  // Hydrate the local edit map from the server's `existing` array on
  // first load. Done in a memo + effect-free check so the component
  // doesn't double-update during fast renders.
  const initialised = useMemo(() => {
    if (!survey) return false
    const map = new Map<string, StudySurveyAnswer>()
    for (const a of survey.existing) {
      map.set(a.question_id, a)
    }
    setAnswers(map)
    return true
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [survey?.kind])
  void initialised

  const mutation = useMutation({
    mutationFn: (payload: { answers: StudySurveyAnswer[] }) =>
      api.post<{ stage: string; current_task_index: number }>(
        `/courses/${courseId}/study/survey/${kind}`,
        payload,
      ),
    onSuccess: () => {
      queryClient.invalidateQueries({
        queryKey: ["courses", courseId, "study", "state"],
      })
      queryClient.invalidateQueries({
        queryKey: ["courses", courseId, "study", "survey", kind],
      })
    },
    onError: (e) => setSubmitError(e),
  })

  if (isLoading) {
    return (
      <Card className="mx-auto my-8 max-w-3xl space-y-3 p-6">
        <Skeleton className="h-6 w-1/2" />
        <Skeleton className="h-20 w-full" />
        <Skeleton className="h-20 w-full" />
      </Card>
    )
  }

  if (loadError || !survey) {
    return (
      <Card className="mx-auto my-8 max-w-3xl p-6">
        <p role="alert" className="text-sm text-destructive">
          {formatError(loadError)}
        </p>
      </Card>
    )
  }

  const titleKey = kind === "pre" ? "preSurvey.title" : "postSurvey.title"
  const descKey =
    kind === "pre" ? "preSurvey.description" : "postSurvey.description"
  const tOptional = t("survey.optionalLabel")

  const setLikert = (q: StudySurveyQuestion, value: number) => {
    setValidationError(null)
    const next = new Map(answers)
    next.set(q.id, {
      question_id: q.id,
      likert_value: value,
      free_text_value: null,
    })
    setAnswers(next)
  }

  const setFreeText = (q: StudySurveyQuestion, value: string) => {
    setValidationError(null)
    const next = new Map(answers)
    next.set(q.id, {
      question_id: q.id,
      likert_value: null,
      free_text_value: value,
    })
    setAnswers(next)
  }

  const validateAndSubmit = () => {
    setSubmitError(null)
    const out: StudySurveyAnswer[] = []
    for (const q of survey.questions) {
      // Section headings are display-only; never validated, never sent.
      if (q.kind === "section_heading") continue

      const a = answers.get(q.id)
      const hasAnswer =
        a != null &&
        ((q.kind === "likert" && a.likert_value !== null) ||
          (q.kind === "free_text" &&
            a.free_text_value != null &&
            a.free_text_value.trim() !== ""))

      if (!hasAnswer) {
        if (q.is_required) {
          setValidationError(t("survey.validation.incomplete"))
          return
        }
        // Optional + unanswered: skip without sending.
        continue
      }

      if (q.kind === "likert") {
        if (
          a!.likert_value === null ||
          q.likert_min === null ||
          q.likert_max === null ||
          a!.likert_value! < q.likert_min ||
          a!.likert_value! > q.likert_max
        ) {
          setValidationError(t("survey.validation.outOfRange"))
          return
        }
      }
      out.push(a!)
    }
    mutation.mutate({ answers: out })
  }

  return (
    <Card className="mx-auto my-8 max-w-3xl space-y-6 p-6">
      <div>
        <h1 className="text-2xl font-semibold">{t(titleKey)}</h1>
        <p className="text-sm text-muted-foreground mt-1">{t(descKey)}</p>
      </div>

      <div className="space-y-6">
        {survey.questions.map((q) => {
          if (q.kind === "section_heading") {
            return (
              <div
                key={q.id}
                className="border-t pt-6 first:border-none first:pt-0"
              >
                <h2 className="text-lg font-semibold">{q.prompt}</h2>
              </div>
            )
          }
          return (
            <div key={q.id} className="space-y-2">
              <p className="font-medium">
                {q.prompt}
                {!q.is_required && (
                  <span className="ml-2 text-xs text-muted-foreground font-normal">
                    ({tOptional})
                  </span>
                )}
              </p>
              {q.kind === "likert" ? (
                <LikertScale
                  question={q}
                  value={answers.get(q.id)?.likert_value ?? null}
                  disabled={mutation.isPending}
                  onChange={(v) => setLikert(q, v)}
                />
              ) : (
                <Textarea
                  value={answers.get(q.id)?.free_text_value ?? ""}
                  onChange={(e) => setFreeText(q, e.target.value)}
                  placeholder={t("survey.freeTextPlaceholder")}
                  disabled={mutation.isPending}
                  rows={4}
                />
              )}
            </div>
          )
        })}
      </div>

      {validationError !== null && (
        <p role="alert" className="text-sm text-destructive">
          {validationError}
        </p>
      )}
      {submitError !== null && (
        <p role="alert" className="text-sm text-destructive">
          {formatError(submitError)}
        </p>
      )}

      <div className="flex justify-end">
        <Button onClick={validateAndSubmit} disabled={mutation.isPending}>
          {mutation.isPending
            ? t("survey.submittingButton")
            : t("survey.submitButton")}
        </Button>
      </div>
    </Card>
  )
}

function LikertScale({
  question,
  value,
  onChange,
  disabled,
}: {
  question: StudySurveyQuestion
  value: number | null
  onChange: (v: number) => void
  disabled?: boolean
}) {
  const { t } = useTranslation("study")
  const min = question.likert_min ?? 1
  const max = question.likert_max ?? 5
  const ticks: number[] = []
  for (let i = min; i <= max; i++) ticks.push(i)
  return (
    <div
      role="radiogroup"
      aria-label={t("survey.likertSelectAria", {
        prompt: question.prompt,
        min,
        max,
      })}
      className="space-y-1"
    >
      <div className="flex justify-between text-xs text-muted-foreground">
        <span>{question.likert_min_label ?? min}</span>
        <span>{question.likert_max_label ?? max}</span>
      </div>
      <div className="flex flex-wrap gap-2">
        {ticks.map((v) => (
          <label
            key={v}
            className={`flex h-10 w-10 cursor-pointer items-center justify-center rounded-md border text-sm transition-colors ${
              value === v
                ? "bg-primary text-primary-foreground border-primary"
                : "bg-background hover:bg-muted"
            } ${disabled ? "opacity-50 pointer-events-none" : ""}`}
          >
            <input
              type="radio"
              name={`likert-${question.id}`}
              className="sr-only"
              checked={value === v}
              onChange={() => onChange(v)}
              disabled={disabled}
            />
            {v}
          </label>
        ))}
      </div>
    </div>
  )
}
