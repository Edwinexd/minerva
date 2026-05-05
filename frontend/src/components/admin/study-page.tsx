import { useMemo, useState } from "react"
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query"
import { useTranslation } from "react-i18next"
import { api } from "@/lib/api"
import {
  adminStudyConfigQuery,
  adminStudyParticipantsQuery,
  coursesQuery,
} from "@/lib/queries"
import type {
  AdminStudyConfig,
  AdminStudyConfigPutBody,
  AdminStudyQuestionConfig,
  AdminStudyTask,
} from "@/lib/queries"
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { Textarea } from "@/components/ui/textarea"
import { Skeleton } from "@/components/ui/skeleton"
import { Badge } from "@/components/ui/badge"
import { Checkbox } from "@/components/ui/checkbox"
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select"
import { useApiErrorMessage } from "@/lib/use-api-error"

/**
 * Admin tab for study mode. Lists courses with the `study_mode`
 * flag enabled (set per-course via the existing course management
 * tab); selecting one opens an inline config + participants panel.
 *
 * The question editor is intentionally read-only; for the current
 * Aegis evaluation surveys are seeded via a script
 * (`scripts/seed_aegis_study.sql` in Phase 5) and the researcher
 * doesn't need a per-question editor here. Researchers can still
 * tweak the consent copy, thank-you copy, number-of-tasks, and the
 * task list (title + description) live without rerunning the seed.
 */
export function AdminStudyPanel() {
  const { t } = useTranslation("admin")
  const { data: courses, isLoading } = useQuery(coursesQuery)
  const [selected, setSelected] = useState<string | null>(null)

  const studyCourses = useMemo(
    () => (courses ?? []).filter((c) => c.feature_flags?.study_mode === true),
    [courses],
  )

  if (isLoading) return <Skeleton className="h-32 w-full" />

  if (studyCourses.length === 0) {
    return (
      <Card>
        <CardHeader>
          <CardTitle>{t("study.title")}</CardTitle>
        </CardHeader>
        <CardContent>
          <p className="text-sm text-muted-foreground">
            {t("study.noCoursesEnabled")}
          </p>
        </CardContent>
      </Card>
    )
  }

  return (
    <div className="space-y-6">
      <Card>
        <CardHeader>
          <CardTitle>{t("study.title")}</CardTitle>
        </CardHeader>
        <CardContent className="space-y-3">
          <p className="text-sm text-muted-foreground">{t("study.intro")}</p>
          <div className="max-w-md">
            <Select
              value={selected ?? ""}
              onValueChange={(v) => setSelected(v || null)}
            >
              <SelectTrigger>
                <SelectValue placeholder={t("study.coursePlaceholder")} />
              </SelectTrigger>
              <SelectContent>
                {studyCourses.map((c) => (
                  <SelectItem key={c.id} value={c.id}>
                    {c.name}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </div>
        </CardContent>
      </Card>

      {selected && (
        <>
          <ConfigPanel courseId={selected} />
          <ParticipantsPanel courseId={selected} />
        </>
      )}
    </div>
  )
}

function ConfigPanel({ courseId }: { courseId: string }) {
  const { t } = useTranslation("admin")
  const formatError = useApiErrorMessage()
  const queryClient = useQueryClient()
  const { data, isLoading, error } = useQuery(adminStudyConfigQuery(courseId))

  // Local edit state, hydrated from server. Mutating fields are
  // tracked individually; the rest of the AdminStudyConfig payload
  // (notably `course_id` + the read-only response_count metadata) is
  // echoed back from `data` at save time.
  const [consentHtml, setConsentHtml] = useState<string | null>(null)
  const [thankYouHtml, setThankYouHtml] = useState<string | null>(null)
  const [numberOfTasks, setNumberOfTasks] = useState<number | null>(null)
  const [tasks, setTasks] = useState<AdminStudyTask[] | null>(null)
  const [preQuestions, setPreQuestions] =
    useState<AdminStudyQuestionConfig[] | null>(null)
  const [postQuestions, setPostQuestions] =
    useState<AdminStudyQuestionConfig[] | null>(null)

  // Hydrate on first load.
  if (data && consentHtml === null) {
    setConsentHtml(data.consent_html)
    setThankYouHtml(data.thank_you_html)
    setNumberOfTasks(data.number_of_tasks)
    setTasks(data.tasks)
    setPreQuestions(data.pre_survey?.questions ?? [])
    setPostQuestions(data.post_survey?.questions ?? [])
  }

  const mutation = useMutation({
    // Note the request type is asymmetric with the response: PUT
    // takes bare question arrays, GET returns survey objects with
    // metadata. See `AdminStudyConfigPutBody`.
    mutationFn: (body: AdminStudyConfigPutBody) =>
      api.put<AdminStudyConfig>(`/admin/study/courses/${courseId}/config`, body),
    onSuccess: () => {
      queryClient.invalidateQueries({
        queryKey: ["admin", "study", "courses", courseId, "config"],
      })
      queryClient.invalidateQueries({ queryKey: ["courses"] })
    },
  })

  // One-shot DM2731 / Aegis preset loader. POSTs to a backend route
  // that has the canonical content baked into Rust; on success we
  // refetch the config and the local edit state re-hydrates from
  // the new server values, so the user sees the seeded content
  // immediately without a page reload.
  const seedMutation = useMutation({
    mutationFn: () =>
      api.post<AdminStudyConfig>(
        `/admin/study/courses/${courseId}/seed-dm2731`,
        {},
      ),
    onSuccess: () => {
      // Reset the local edit cursor so the next render hydrates
      // from the freshly returned config rather than overlaying
      // the just-seeded values with stale local edits.
      setConsentHtml(null)
      setThankYouHtml(null)
      setNumberOfTasks(null)
      setTasks(null)
      setPreQuestions(null)
      setPostQuestions(null)
      queryClient.invalidateQueries({
        queryKey: ["admin", "study", "courses", courseId, "config"],
      })
    },
  })

  if (isLoading) return <Skeleton className="h-64 w-full" />
  if (
    error ||
    !data ||
    tasks === null ||
    numberOfTasks === null ||
    preQuestions === null ||
    postQuestions === null
  ) {
    return (
      <Card>
        <CardContent className="pt-6">
          <p className="text-sm text-destructive">
            {formatError(error ?? new Error("config missing"))}
          </p>
        </CardContent>
      </Card>
    )
  }

  const setTask = (idx: number, patch: Partial<AdminStudyTask>) => {
    const next = tasks.map((t, i) => (i === idx ? { ...t, ...patch } : t))
    setTasks(next)
  }

  const addTask = () => {
    const next = [
      ...tasks,
      { task_index: tasks.length, title: "", description: "" },
    ]
    setTasks(next)
    setNumberOfTasks(next.length)
  }

  const removeTask = (idx: number) => {
    const next = tasks
      .filter((_, i) => i !== idx)
      .map((t, i) => ({ ...t, task_index: i }))
    setTasks(next)
    setNumberOfTasks(next.length)
  }

  const onSave = () => {
    // Backend PUT shape: bare question arrays, not survey-config
    // objects. Replaces consent / thank-you / tasks / both surveys
    // atomically (transactional delete-then-insert per survey).
    mutation.mutate({
      consent_html: consentHtml ?? "",
      thank_you_html: thankYouHtml ?? "",
      number_of_tasks: numberOfTasks,
      completion_gate_kind: data.completion_gate_kind,
      tasks,
      pre_survey: preQuestions,
      post_survey: postQuestions,
    })
  }

  return (
    <Card>
      <CardHeader>
        <CardTitle>{t("study.configTitle")}</CardTitle>
      </CardHeader>
      <CardContent className="space-y-6">
        {data.has_in_flight_participants && (
          <div className="rounded-md border border-amber-300 bg-amber-50 p-3 text-sm dark:border-amber-800 dark:bg-amber-950/40">
            {t("study.inFlightWarning")}
          </div>
        )}

        <div className="rounded-md border border-dashed p-3 space-y-2">
          <div className="flex items-center justify-between gap-3">
            <div className="space-y-0.5">
              <p className="text-sm font-medium">{t("study.seedDm2731Title")}</p>
              <p className="text-xs text-muted-foreground">
                {t("study.seedDm2731Description")}
              </p>
            </div>
            <Button
              variant="outline"
              size="sm"
              onClick={() => {
                if (
                  data.has_in_flight_participants ||
                  (preQuestions && preQuestions.length > 0) ||
                  (postQuestions && postQuestions.length > 0) ||
                  (tasks && tasks.length > 0)
                ) {
                  if (!window.confirm(t("study.seedDm2731Confirm"))) return
                }
                seedMutation.mutate()
              }}
              disabled={seedMutation.isPending}
            >
              {seedMutation.isPending
                ? t("study.seedDm2731Loading")
                : t("study.seedDm2731Button")}
            </Button>
          </div>
          {seedMutation.error !== null && (
            <p role="alert" className="text-sm text-destructive">
              {formatError(seedMutation.error)}
            </p>
          )}
        </div>

        <div className="space-y-2">
          <label className="text-sm font-medium">
            {t("study.consentLabel")}
          </label>
          <Textarea
            value={consentHtml ?? ""}
            onChange={(e) => setConsentHtml(e.target.value)}
            rows={10}
            placeholder={t("study.consentPlaceholder")}
          />
          <p className="text-xs text-muted-foreground">
            {t("study.markdownHelp")}
          </p>
        </div>

        <div className="space-y-2">
          <label className="text-sm font-medium">
            {t("study.thankYouLabel")}
          </label>
          <Textarea
            value={thankYouHtml ?? ""}
            onChange={(e) => setThankYouHtml(e.target.value)}
            rows={6}
            placeholder={t("study.thankYouPlaceholder")}
          />
          <p className="text-xs text-muted-foreground">
            {t("study.markdownHelp")}
          </p>
        </div>

        <div className="space-y-3">
          <div className="flex items-center justify-between">
            <h3 className="text-sm font-medium">
              {t("study.tasksLabel", { count: tasks.length })}
            </h3>
            <Button variant="outline" size="sm" onClick={addTask}>
              {t("study.addTask")}
            </Button>
          </div>
          {tasks.map((task, idx) => (
            <div key={idx} className="space-y-2 rounded-md border p-3">
              <div className="flex items-center gap-2">
                <Badge variant="outline">
                  {t("study.taskBadge", { n: idx + 1 })}
                </Badge>
                <Input
                  value={task.title}
                  onChange={(e) => setTask(idx, { title: e.target.value })}
                  placeholder={t("study.taskTitlePlaceholder")}
                  className="flex-1"
                />
                <Button
                  variant="ghost"
                  size="sm"
                  onClick={() => removeTask(idx)}
                >
                  {t("study.removeTask")}
                </Button>
              </div>
              <Textarea
                value={task.description}
                onChange={(e) => setTask(idx, { description: e.target.value })}
                placeholder={t("study.taskDescriptionPlaceholder")}
                rows={4}
              />
            </div>
          ))}
        </div>

        <SurveyEditor
          label={t("study.preSurveyLabel")}
          questions={preQuestions}
          onChange={setPreQuestions}
          responseCount={data.pre_survey?.response_count ?? 0}
        />
        <SurveyEditor
          label={t("study.postSurveyLabel")}
          questions={postQuestions}
          onChange={setPostQuestions}
          responseCount={data.post_survey?.response_count ?? 0}
        />

        {mutation.error !== null && (
          <p role="alert" className="text-sm text-destructive">
            {formatError(mutation.error)}
          </p>
        )}

        <div className="flex justify-end gap-2">
          <Button onClick={onSave} disabled={mutation.isPending}>
            {mutation.isPending ? t("study.saving") : t("study.save")}
          </Button>
        </div>
      </CardContent>
    </Card>
  )
}

/**
 * Per-survey question editor. Wholly replaces the read-only summary
 * the previous version of this page had; researchers configure
 * everything via the UI now (no seed-script dependency).
 *
 * State is hoisted to the parent ConfigPanel so the Save button
 * commits the whole config (consent + tasks + both surveys) in one
 * PUT. The backend replaces each survey atomically (delete + insert
 * under one transaction), so editing here doesn't leave the survey
 * in a half-applied state if the request fails.
 */
function SurveyEditor({
  label,
  questions,
  onChange,
  responseCount,
}: {
  label: string
  questions: AdminStudyQuestionConfig[]
  onChange: (next: AdminStudyQuestionConfig[]) => void
  responseCount: number
}) {
  const { t } = useTranslation("admin")

  const setQ = (idx: number, patch: Partial<AdminStudyQuestionConfig>) => {
    const next = questions.map((q, i) => (i === idx ? { ...q, ...patch } : q))
    onChange(next)
  }

  const addQuestion = (kind: AdminStudyQuestionConfig["kind"]) => {
    const base: AdminStudyQuestionConfig = {
      kind,
      prompt: "",
      likert_min: kind === "likert" ? 1 : null,
      likert_max: kind === "likert" ? 5 : null,
      likert_min_label: kind === "likert" ? "" : null,
      likert_max_label: kind === "likert" ? "" : null,
      is_required: kind !== "section_heading",
      kill_on_value: null,
    }
    onChange([...questions, base])
  }

  const removeQuestion = (idx: number) => {
    onChange(questions.filter((_, i) => i !== idx))
  }

  const move = (idx: number, dir: -1 | 1) => {
    const target = idx + dir
    if (target < 0 || target >= questions.length) return
    const next = questions.slice()
    ;[next[idx], next[target]] = [next[target], next[idx]]
    onChange(next)
  }

  return (
    <div className="space-y-3 rounded-md border p-4">
      <div className="flex items-center justify-between">
        <h3 className="text-sm font-medium">{label}</h3>
        <Badge variant="secondary">
          {t("study.responseCount", { count: responseCount })}
        </Badge>
      </div>

      {responseCount > 0 && (
        <p className="rounded border border-amber-300 bg-amber-50 p-2 text-xs dark:border-amber-800 dark:bg-amber-950/40">
          {t("study.surveyHasResponsesWarning")}
        </p>
      )}

      {questions.length === 0 ? (
        <p className="text-xs text-muted-foreground">
          {t("study.noQuestionsYet")}
        </p>
      ) : (
        <ol className="space-y-3">
          {questions.map((q, idx) => (
            <li key={idx} className="space-y-2 rounded border p-3">
              <div className="flex items-center gap-2">
                <Badge variant="outline" className="font-mono text-xs">
                  {idx + 1}
                </Badge>
                <Select
                  value={q.kind}
                  onValueChange={(v) =>
                    setQ(idx, {
                      kind: v as AdminStudyQuestionConfig["kind"],
                      // Coerce to legal shape for the new kind so the
                      // backend's CHECK constraint doesn't reject the
                      // save. Likert metadata is null'd out when
                      // switching to free_text or section_heading;
                      // section_heading is forced optional.
                      likert_min: v === "likert" ? (q.likert_min ?? 1) : null,
                      likert_max: v === "likert" ? (q.likert_max ?? 5) : null,
                      likert_min_label:
                        v === "likert" ? (q.likert_min_label ?? "") : null,
                      likert_max_label:
                        v === "likert" ? (q.likert_max_label ?? "") : null,
                      is_required:
                        v === "section_heading" ? false : q.is_required,
                      kill_on_value: v === "likert" ? q.kill_on_value : null,
                    })
                  }
                >
                  <SelectTrigger className="w-44">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem value="likert">
                      {t("study.questionKind.likert")}
                    </SelectItem>
                    <SelectItem value="free_text">
                      {t("study.questionKind.free_text")}
                    </SelectItem>
                    <SelectItem value="section_heading">
                      {t("study.questionKind.section_heading")}
                    </SelectItem>
                  </SelectContent>
                </Select>
                <div className="ml-auto flex items-center gap-1">
                  <Button
                    variant="ghost"
                    size="sm"
                    onClick={() => move(idx, -1)}
                    disabled={idx === 0}
                    aria-label={t("study.moveUp")}
                  >
                    {"↑"}
                  </Button>
                  <Button
                    variant="ghost"
                    size="sm"
                    onClick={() => move(idx, 1)}
                    disabled={idx === questions.length - 1}
                    aria-label={t("study.moveDown")}
                  >
                    {"↓"}
                  </Button>
                  <Button
                    variant="ghost"
                    size="sm"
                    onClick={() => removeQuestion(idx)}
                  >
                    {t("study.removeQuestion")}
                  </Button>
                </div>
              </div>

              <Textarea
                value={q.prompt}
                onChange={(e) => setQ(idx, { prompt: e.target.value })}
                placeholder={t("study.questionPromptPlaceholder")}
                rows={2}
              />

              {q.kind === "likert" && (
                <div className="grid grid-cols-2 gap-3 sm:grid-cols-4">
                  <div className="space-y-1">
                    <label className="text-xs text-muted-foreground">
                      {t("study.likertMin")}
                    </label>
                    <Input
                      type="number"
                      value={q.likert_min ?? ""}
                      onChange={(e) =>
                        setQ(idx, {
                          likert_min: e.target.value === ""
                            ? null
                            : Number(e.target.value),
                        })
                      }
                    />
                  </div>
                  <div className="space-y-1">
                    <label className="text-xs text-muted-foreground">
                      {t("study.likertMax")}
                    </label>
                    <Input
                      type="number"
                      value={q.likert_max ?? ""}
                      onChange={(e) =>
                        setQ(idx, {
                          likert_max: e.target.value === ""
                            ? null
                            : Number(e.target.value),
                        })
                      }
                    />
                  </div>
                  <div className="space-y-1">
                    <label className="text-xs text-muted-foreground">
                      {t("study.likertMinLabel")}
                    </label>
                    <Input
                      value={q.likert_min_label ?? ""}
                      onChange={(e) =>
                        setQ(idx, { likert_min_label: e.target.value })
                      }
                      placeholder={t("study.likertMinPlaceholder")}
                    />
                  </div>
                  <div className="space-y-1">
                    <label className="text-xs text-muted-foreground">
                      {t("study.likertMaxLabel")}
                    </label>
                    <Input
                      value={q.likert_max_label ?? ""}
                      onChange={(e) =>
                        setQ(idx, { likert_max_label: e.target.value })
                      }
                      placeholder={t("study.likertMaxPlaceholder")}
                    />
                  </div>
                </div>
              )}

              {q.kind !== "section_heading" && (
                <label className="flex items-center gap-2 text-sm">
                  <Checkbox
                    checked={q.is_required}
                    onCheckedChange={(v) =>
                      setQ(idx, { is_required: v === true })
                    }
                  />
                  {t("study.requiredLabel")}
                </label>
              )}

              {q.kind === "likert" && (
                <div className="space-y-1">
                  <label className="text-xs text-muted-foreground">
                    {t("study.killOnValueLabel")}
                  </label>
                  <Input
                    type="number"
                    value={q.kill_on_value ?? ""}
                    onChange={(e) =>
                      setQ(idx, {
                        kill_on_value: e.target.value === ""
                          ? null
                          : Number(e.target.value),
                      })
                    }
                    placeholder={t("study.killOnValuePlaceholder")}
                  />
                  <p className="text-xs text-muted-foreground">
                    {t("study.killOnValueHelp")}
                  </p>
                </div>
              )}
            </li>
          ))}
        </ol>
      )}

      <div className="flex flex-wrap gap-2">
        <Button
          variant="outline"
          size="sm"
          onClick={() => addQuestion("likert")}
        >
          {t("study.addLikert")}
        </Button>
        <Button
          variant="outline"
          size="sm"
          onClick={() => addQuestion("free_text")}
        >
          {t("study.addFreeText")}
        </Button>
        <Button
          variant="outline"
          size="sm"
          onClick={() => addQuestion("section_heading")}
        >
          {t("study.addSectionHeading")}
        </Button>
      </div>
    </div>
  )
}

function ParticipantsPanel({ courseId }: { courseId: string }) {
  const { t } = useTranslation("admin")
  const formatError = useApiErrorMessage()
  const { data, isLoading, error } = useQuery(
    adminStudyParticipantsQuery(courseId),
  )

  // Download is a normal anchor click rather than fetch; the
  // browser handles the streaming response and the file save.
  // Cookie auth + dev-user header come along automatically for
  // same-origin requests.
  const downloadUrl = `/api/admin/study/courses/${courseId}/export.jsonl`

  return (
    <Card>
      <CardHeader>
        <CardTitle>{t("study.participantsTitle")}</CardTitle>
      </CardHeader>
      <CardContent className="space-y-4">
        <div className="flex justify-end">
          <a href={downloadUrl} download>
            <Button variant="outline" size="sm">
              {t("study.downloadJsonl")}
            </Button>
          </a>
        </div>

        {isLoading ? (
          <Skeleton className="h-32 w-full" />
        ) : error ? (
          <p role="alert" className="text-sm text-destructive">
            {formatError(error)}
          </p>
        ) : !data || data.length === 0 ? (
          <p className="text-sm text-muted-foreground">
            {t("study.noParticipants")}
          </p>
        ) : (
          <div className="overflow-x-auto">
            <table className="w-full text-sm">
              <thead>
                <tr className="border-b text-left">
                  <th className="py-2 pr-4 font-medium">
                    {t("study.cols.participant")}
                  </th>
                  <th className="py-2 pr-4 font-medium">
                    {t("study.cols.stage")}
                  </th>
                  <th className="py-2 pr-4 font-medium">
                    {t("study.cols.task")}
                  </th>
                  <th className="py-2 pr-4 font-medium">
                    {t("study.cols.consented")}
                  </th>
                  <th className="py-2 pr-4 font-medium">
                    {t("study.cols.preCompleted")}
                  </th>
                  <th className="py-2 pr-4 font-medium">
                    {t("study.cols.postCompleted")}
                  </th>
                  <th className="py-2 font-medium">
                    {t("study.cols.lockedOut")}
                  </th>
                </tr>
              </thead>
              <tbody>
                {data.map((p) => (
                  <tr key={p.user_id} className="border-b">
                    <td className="py-2 pr-4">
                      <div className="font-medium">
                        {p.display_name ?? p.eppn ?? p.user_id.slice(0, 8)}
                      </div>
                      {p.display_name && p.eppn && (
                        <div className="text-xs text-muted-foreground">
                          {p.eppn}
                        </div>
                      )}
                    </td>
                    <td className="py-2 pr-4">
                      <Badge variant="outline">{p.stage}</Badge>
                    </td>
                    <td className="py-2 pr-4">{p.current_task_index}</td>
                    <td className="py-2 pr-4">{formatTs(p.consented_at)}</td>
                    <td className="py-2 pr-4">
                      {formatTs(p.pre_survey_completed_at)}
                    </td>
                    <td className="py-2 pr-4">
                      {formatTs(p.post_survey_completed_at)}
                    </td>
                    <td className="py-2">{formatTs(p.locked_out_at)}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
      </CardContent>
    </Card>
  )
}

function formatTs(s: string | null): string {
  if (!s) return "-"
  // Compact ISO-without-seconds for the table; the full timestamp
  // is in the JSONL export anyway.
  try {
    return new Date(s).toISOString().slice(0, 16).replace("T", " ")
  } catch {
    return s
  }
}
