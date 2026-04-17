import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query"
import { useTranslation } from "react-i18next"
import { courseQuery, modelsQuery, embeddingBenchmarksQuery } from "@/lib/queries"
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
import { useState } from "react"
import type { Course } from "@/lib/types"

export function ConfigPage({ useParams }: { useParams: () => { courseId: string } }) {
  const { courseId } = useParams()
  const { data: course } = useQuery(courseQuery(courseId))
  if (!course) return null
  return <CourseConfigForm course={course} />
}

function CourseConfigForm({ course }: { course: Course }) {
  const queryClient = useQueryClient()
  const { t } = useTranslation("teacher")
  const { t: tCommon } = useTranslation("common")
  const formatError = useApiErrorMessage()
  const { data: modelsData } = useQuery(modelsQuery)
  const { data: benchmarksData } = useQuery(embeddingBenchmarksQuery)
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

  const mutation = useMutation({
    mutationFn: (data: Record<string, unknown>) =>
      api.put<Course>(`/courses/${course.id}`, data),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["courses"] })
    },
  })

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
            mutation.mutate({
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
                  {[
                    { id: "sentence-transformers/all-MiniLM-L6-v2", name: "all-MiniLM-L6-v2", dims: 384, descKey: "config.embeddingModels.miniLmDesc" },
                    { id: "BAAI/bge-small-en-v1.5", name: "BGE Small EN v1.5", dims: 384, descKey: "config.embeddingModels.bgeSmallDesc" },
                    { id: "BAAI/bge-base-en-v1.5", name: "BGE Base EN v1.5", dims: 768, descKey: "config.embeddingModels.bgeBaseDesc" },
                    { id: "nomic-ai/nomic-embed-text-v1.5", name: "Nomic Embed Text v1.5", dims: 768, descKey: "config.embeddingModels.nomicDesc" },
                  ].map((m) => {
                    const bench = benchmarksData?.benchmarks.find((b) => b.model === m.id)
                    const speed = bench ? t("config.embeddingSpeedSuffix", { value: Math.round(bench.embeddings_per_second) }) : ""
                    return (
                      <SelectItem key={m.id} value={m.id}>
                        {t("config.embeddingModelItem", { name: m.name, dims: m.dims, desc: t(m.descKey), speed })}
                      </SelectItem>
                    )
                  })}
                </SelectContent>
              </Select>
              <p className="text-xs text-muted-foreground">
                {t("config.embeddingModelHelp")}
                {benchmarksData?.benchmarks.length ? t("config.embeddingModelHelpSpeed") : ""}
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
      </CardContent>
    </Card>
  )
}
