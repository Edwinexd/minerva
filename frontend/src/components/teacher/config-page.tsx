import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query"
import { useTranslation } from "react-i18next"
import {
  courseQuery,
  modelsQuery,
  embeddingModelsQuery,
  courseKgTokenUsageQuery,
} from "@/lib/queries"
import { api } from "@/lib/api"
import { useApiErrorMessage } from "@/lib/use-api-error"
import { Button } from "@/components/ui/button"
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"
import { Slider } from "@/components/ui/slider"
import { Textarea } from "@/components/ui/textarea"
import { Separator } from "@/components/ui/separator"
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select"
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog"
import { useState } from "react"
import type { Course } from "@/lib/types"
import { MODEL_DISPLAY } from "@/lib/embedding-models"

// MODEL_DISPLAY now lives in @/lib/embedding-models so the admin
// courses table can use the same friendly names without duplicating
// the catalog.

export function ConfigPage({ useParams }: { useParams: () => { courseId: string } }) {
  const { courseId } = useParams()
  const { data: course } = useQuery(courseQuery(courseId))
  if (!course) return null
  return (
    <div className="space-y-6">
      <CourseConfigForm course={course} />
      <KgTokenUsageCard courseId={courseId} />
    </div>
  )
}

/**
 * Per-course KG / extraction-guard token-spend card. One row per
 * (category, model) over the last 30 days, plus a totals line.
 * No spending limits enforced -- this is observability only --
 * but the data feeds straight from `course_token_usage`, so once
 * we add limits it'll be the same numbers shown here.
 */
function KgTokenUsageCard({ courseId }: { courseId: string }) {
  const { t } = useTranslation("teacher")
  const { data, isLoading } = useQuery(courseKgTokenUsageQuery(courseId))
  if (isLoading || !data) {
    return (
      <Card>
        <CardHeader>
          <CardTitle>{t("kgTokenUsage.title")}</CardTitle>
          <CardDescription>{t("kgTokenUsage.loading")}</CardDescription>
        </CardHeader>
      </Card>
    )
  }

  const totalPrompt = data.rows.reduce((s, r) => s + r.prompt_tokens, 0)
  const totalCompletion = data.rows.reduce((s, r) => s + r.completion_tokens, 0)
  const totalCalls = data.rows.reduce((s, r) => s + r.call_count, 0)
  const sinceDate = new Date(data.since)

  return (
    <Card>
      <CardHeader>
        <CardTitle>{t("kgTokenUsage.title")}</CardTitle>
        <CardDescription>
          {t("kgTokenUsage.subtitle", {
            since: sinceDate.toISOString().slice(0, 10),
          })}
        </CardDescription>
      </CardHeader>
      <CardContent>
        {data.rows.length === 0 ? (
          <p className="text-sm text-muted-foreground">
            {t("kgTokenUsage.empty")}
          </p>
        ) : (
          <div className="overflow-x-auto">
            <table className="w-full text-sm">
              <thead>
                <tr className="text-left border-b text-muted-foreground">
                  <th className="py-1 pr-3 font-medium">
                    {t("kgTokenUsage.colCategory")}
                  </th>
                  <th className="py-1 pr-3 font-medium">
                    {t("kgTokenUsage.colModel")}
                  </th>
                  <th className="py-1 pr-3 font-medium text-right">
                    {t("kgTokenUsage.colCalls")}
                  </th>
                  <th className="py-1 pr-3 font-medium text-right">
                    {t("kgTokenUsage.colPrompt")}
                  </th>
                  <th className="py-1 font-medium text-right">
                    {t("kgTokenUsage.colCompletion")}
                  </th>
                </tr>
              </thead>
              <tbody>
                {data.rows.map((r) => (
                  <tr
                    key={`${r.category}-${r.model}`}
                    className="border-b last:border-b-0"
                  >
                    <td className="py-1 pr-3">
                      {t(`kgTokenUsage.category.${r.category}`, r.category)}
                    </td>
                    <td className="py-1 pr-3 font-mono text-xs">{r.model}</td>
                    <td className="py-1 pr-3 text-right tabular-nums">
                      {r.call_count.toLocaleString()}
                    </td>
                    <td className="py-1 pr-3 text-right tabular-nums">
                      {r.prompt_tokens.toLocaleString()}
                    </td>
                    <td className="py-1 text-right tabular-nums">
                      {r.completion_tokens.toLocaleString()}
                    </td>
                  </tr>
                ))}
                <tr className="font-medium">
                  <td className="py-1 pr-3" colSpan={2}>
                    {t("kgTokenUsage.total")}
                  </td>
                  <td className="py-1 pr-3 text-right tabular-nums">
                    {totalCalls.toLocaleString()}
                  </td>
                  <td className="py-1 pr-3 text-right tabular-nums">
                    {totalPrompt.toLocaleString()}
                  </td>
                  <td className="py-1 text-right tabular-nums">
                    {totalCompletion.toLocaleString()}
                  </td>
                </tr>
              </tbody>
            </table>
          </div>
        )}
      </CardContent>
    </Card>
  )
}

function CourseConfigForm({ course }: { course: Course }) {
  const queryClient = useQueryClient()
  const { t } = useTranslation("teacher")
  const { t: tCommon } = useTranslation("common")
  const formatError = useApiErrorMessage()
  const { data: modelsData } = useQuery(modelsQuery)
  // Backend filters this list to admin-enabled catalog rows. If an
  // admin disabled the model this course is currently on, it won't be
  // here -- we patch the current course's model back into the option
  // list below so the teacher still sees what they're using rather
  // than an empty trigger. They can save unrelated config without
  // re-picking; only an actual model *change* hits the
  // `local_embedding_model_disabled` server-side check.
  const { data: embeddingModelsData } = useQuery(embeddingModelsQuery)
  const readOnly = course.my_role === "ta"
  const [name, setName] = useState(course.name)
  const [description, setDescription] = useState(course.description || "")
  const [contextRatio, setContextRatio] = useState(course.context_ratio)
  const [temperature, setTemperature] = useState(course.temperature)
  const [model, setModel] = useState(course.model)
  const [systemPrompt, setSystemPrompt] = useState(course.system_prompt || "")
  const [maxChunks, setMaxChunks] = useState(course.max_chunks)
  const [minScore, setMinScore] = useState(course.min_score)
  const [strategy, setStrategy] = useState(course.strategy)
  const [embeddingProvider, setEmbeddingProvider] = useState(course.embedding_provider)
  const [embeddingModel, setEmbeddingModel] = useState(course.embedding_model)
  const [dailyTokenLimit, setDailyTokenLimit] = useState(course.daily_token_limit)
  // Two-step save: any provider/model change opens this dialog so the
  // teacher acknowledges the re-ingestion. Other field changes save
  // immediately. The dialog is keyed off the *baseline* values from
  // `course` so a subsequent save without further embedding edits
  // doesn't re-prompt.
  const [pendingRotate, setPendingRotate] = useState(false)

  const mutation = useMutation({
    mutationFn: (data: Record<string, unknown>) =>
      api.put<Course>(`/courses/${course.id}`, data),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["courses"] })
      setPendingRotate(false)
    },
  })

  const buildPayload = () => ({
    name,
    description: description || null,
    context_ratio: contextRatio,
    temperature,
    model,
    system_prompt: systemPrompt || null,
    max_chunks: maxChunks,
    min_score: minScore,
    strategy,
    embedding_provider: embeddingProvider,
    embedding_model: embeddingModel,
    daily_token_limit: dailyTokenLimit,
  })

  // The backend only rotates when provider or model differ from the
  // currently-persisted values, so mirror that condition here. A
  // provider switch to "openai" canonicalizes the model server-side
  // (text-embedding-3-small) -- treat that as a rotation too because
  // the persisted model name will change even if the dropdown didn't.
  const willRotate =
    embeddingProvider !== course.embedding_provider ||
    embeddingModel !== course.embedding_model

  return (
    <Card>
      <CardHeader>
        <CardTitle>{t("config.title")}</CardTitle>
        <CardDescription>
          {t("config.description")}
        </CardDescription>
      </CardHeader>
      <CardContent>
        {readOnly && (
          <p className="text-sm text-muted-foreground mb-4">
            {t("config.readOnly")}
          </p>
        )}
        <form
          className="space-y-6"
          onSubmit={(e) => {
            e.preventDefault()
            if (willRotate) {
              // Defer the actual PUT until the teacher confirms in
              // the AlertDialog below. Without this gate a misclick
              // on the model dropdown silently re-embeds an entire
              // course's worth of documents.
              setPendingRotate(true)
              return
            }
            mutation.mutate(buildPayload())
          }}
        >
          <div className="space-y-2">
            <Label htmlFor="name">{t("config.nameLabel")}</Label>
            <Input
              id="name"
              value={name}
              onChange={(e) => setName(e.target.value)}
            />
          </div>

          <div className="space-y-2">
            <Label htmlFor="description">{t("config.descriptionLabel")}</Label>
            <Textarea
              id="description"
              value={description}
              onChange={(e) => setDescription(e.target.value)}
            />
          </div>

          <Separator />

          <div className="space-y-2">
            <Label>
              {t("config.contextRatioLabel", { percent: Math.round(contextRatio * 100) })}
            </Label>
            <Slider
              value={[contextRatio]}
              onValueChange={(v) => setContextRatio(Array.isArray(v) ? v[0] : v)}
              min={0.1}
              max={0.95}
              step={0.05}
            />
            <p className="text-xs text-muted-foreground">
              {t("config.contextRatioHelp")}
            </p>
          </div>

          <div className="space-y-2">
            <Label>{t("config.temperatureLabel", { value: temperature.toFixed(2) })}</Label>
            <Slider
              value={[temperature]}
              onValueChange={(v) => setTemperature(Array.isArray(v) ? v[0] : v)}
              min={0}
              max={1}
              step={0.05}
            />
          </div>

          <div className="space-y-2">
            <Label>{t("config.modelLabel")}</Label>
            <Select value={model} onValueChange={(v) => v && setModel(v)}>
              <SelectTrigger className="w-full truncate">
                <SelectValue placeholder={t("config.modelPlaceholder")} />
              </SelectTrigger>
              <SelectContent>
                {modelsData?.models.map((m) => (
                  <SelectItem key={m.id} value={m.id}>
                    {m.name}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </div>

          <div className="space-y-2">
            <Label>{t("config.strategyLabel")}</Label>
            <Select value={strategy} onValueChange={(v) => v && setStrategy(v)}>
              <SelectTrigger className="w-full truncate">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="parallel">
                  {t("config.strategyParallel")}
                </SelectItem>
                <SelectItem value="simple">
                  {t("config.strategySimple")}
                </SelectItem>
                <SelectItem value="flare">
                  {t("config.strategyFlare")}
                </SelectItem>
              </SelectContent>
            </Select>
            <p className="text-xs text-muted-foreground">
              {t("config.strategyHelp")}
            </p>
          </div>

          <div className="space-y-2">
            <Label>{t("config.embeddingProviderLabel")}</Label>
            <Select value={embeddingProvider} onValueChange={(v) => v && setEmbeddingProvider(v)}>
              <SelectTrigger className="w-full truncate">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="local">
                  {t("config.embeddingProviderLocal")}
                </SelectItem>
                <SelectItem value="openai">
                  {t("config.embeddingProviderOpenAI")}
                </SelectItem>
              </SelectContent>
            </Select>
            <p className="text-xs text-muted-foreground">
              {t("config.embeddingProviderHelp")}
            </p>
            {embeddingProvider === "openai" && (
              <div className="rounded-md border border-amber-300 bg-amber-50 px-3 py-2 text-xs text-amber-900 dark:border-amber-800 dark:bg-amber-950/40 dark:text-amber-200">
                {t("config.openaiWarning")}
              </div>
            )}
          </div>

          {embeddingProvider === "local" && (
            <div className="space-y-2">
              <Label>{t("config.embeddingModelLabel")}</Label>
              <Select value={embeddingModel} onValueChange={(v) => v && setEmbeddingModel(v)}>
                <SelectTrigger className="w-full truncate">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  {(() => {
                    // Build the picker option set from the public
                    // /embedding-models feed (admin-enabled only) and
                    // patch in the course's currently-saved model if
                    // it's not in that list -- happens when an admin
                    // has since disabled the model the course is on.
                    // Without the patch the Select trigger renders
                    // blank, which looks broken even though the value
                    // is correct.
                    const apiModels = embeddingModelsData?.models ?? []
                    const ids = new Set(apiModels.map((m) => m.model))
                    const merged = [...apiModels]
                    if (
                      embeddingProvider === "local" &&
                      embeddingModel &&
                      !ids.has(embeddingModel)
                    ) {
                      merged.push({
                        model: embeddingModel,
                        dimensions: 0,
                        benchmark: null,
                      })
                    }
                    return merged.map((m) => {
                      const meta = MODEL_DISPLAY[m.model]
                      const name = meta?.name ?? m.model
                      const desc = meta?.descKey ? t(meta.descKey) : ""
                      const dims = m.dimensions || meta?.dims || 0
                      const speed = m.benchmark
                        ? t("config.embeddingSpeedSuffix", {
                            value: Math.round(
                              m.benchmark.embeddings_per_second,
                            ),
                          })
                        : ""
                      const disabledNote = !ids.has(m.model)
                        ? ` ${t("config.embeddingModelDisabledSuffix")}`
                        : ""
                      return (
                        <SelectItem key={m.model} value={m.model}>
                          {t("config.embeddingModelItem", {
                            name,
                            dims,
                            desc,
                            speed,
                          })}
                          {disabledNote}
                        </SelectItem>
                      )
                    })
                  })()}
                </SelectContent>
              </Select>
              <p className="text-xs text-muted-foreground">
                {t("config.embeddingModelHelp")}
                {embeddingModelsData?.models.some((m) => m.benchmark)
                  ? t("config.embeddingModelHelpSpeed")
                  : ""}
              </p>
            </div>
          )}

          <div className="space-y-2">
            <Label htmlFor="maxChunks">{t("config.maxChunksLabel")}</Label>
            <Input
              id="maxChunks"
              type="number"
              value={maxChunks}
              onChange={(e) => setMaxChunks(parseInt(e.target.value) || 10)}
              min={1}
              max={50}
            />
          </div>

          <div className="space-y-2">
            <Label>{t("config.minScoreLabel", { value: minScore.toFixed(2) })}</Label>
            <Slider
              value={[minScore]}
              onValueChange={(v) => setMinScore(Array.isArray(v) ? v[0] : v)}
              min={0}
              max={1}
              step={0.01}
            />
            <p className="text-xs text-muted-foreground">
              {t("config.minScoreHelp")}
            </p>
          </div>

          <Separator />

          <div className="space-y-2">
            <Label htmlFor="dailyTokenLimit">{t("config.dailyTokenLimitLabel")}</Label>
            <Input
              id="dailyTokenLimit"
              type="number"
              value={dailyTokenLimit}
              onChange={(e) => setDailyTokenLimit(parseInt(e.target.value) || 0)}
              min={0}
            />
            <p className="text-xs text-muted-foreground">
              {t("config.dailyTokenLimitHelp")}
            </p>
          </div>

          <Separator />

          <div className="space-y-2">
            <Label htmlFor="systemPrompt">{t("config.systemPromptLabel")}</Label>
            <Textarea
              id="systemPrompt"
              value={systemPrompt}
              onChange={(e) => setSystemPrompt(e.target.value)}
              placeholder={t("config.systemPromptPlaceholder")}
              rows={4}
            />
          </div>

          {!readOnly && (
            <Button type="submit" disabled={mutation.isPending}>
              {mutation.isPending ? tCommon("actions.saving") : t("config.saveButton")}
            </Button>
          )}
          {mutation.isSuccess && (
            <span className="text-sm text-muted-foreground ml-2">{t("config.savedToast")}</span>
          )}
          {mutation.isError && (
            <p className="text-sm text-destructive">{formatError(mutation.error)}</p>
          )}
        </form>

        <AlertDialog
          open={pendingRotate}
          onOpenChange={(open) => {
            if (!open) setPendingRotate(false)
          }}
        >
          <AlertDialogContent>
            <AlertDialogHeader>
              <AlertDialogTitle>
                {t("config.embeddingRotateConfirmTitle")}
              </AlertDialogTitle>
              <AlertDialogDescription>
                {t("config.embeddingRotateConfirmBody", {
                  fromProvider: course.embedding_provider,
                  fromModel: course.embedding_model,
                  toProvider: embeddingProvider,
                  toModel: embeddingModel,
                })}
              </AlertDialogDescription>
            </AlertDialogHeader>
            <AlertDialogFooter>
              <AlertDialogCancel>
                {t("config.embeddingRotateConfirmCancel")}
              </AlertDialogCancel>
              <AlertDialogAction
                disabled={mutation.isPending}
                onClick={() => mutation.mutate(buildPayload())}
              >
                {mutation.isPending
                  ? tCommon("actions.saving")
                  : t("config.embeddingRotateConfirmAction")}
              </AlertDialogAction>
            </AlertDialogFooter>
          </AlertDialogContent>
        </AlertDialog>
      </CardContent>
    </Card>
  )
}
