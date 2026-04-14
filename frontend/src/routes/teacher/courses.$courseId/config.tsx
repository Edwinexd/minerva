import { createFileRoute } from "@tanstack/react-router"
import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query"
import { courseQuery, modelsQuery, embeddingBenchmarksQuery } from "@/lib/queries"
import { api } from "@/lib/api"
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

export const Route = createFileRoute("/teacher/courses/$courseId/config")({
  component: ConfigPage,
})

function ConfigPage() {
  const { courseId } = Route.useParams()
  const { data: course } = useQuery(courseQuery(courseId))
  if (!course) return null
  return <CourseConfigForm course={course} />
}

function CourseConfigForm({ course }: { course: Course }) {
  const queryClient = useQueryClient()
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
        <CardTitle>Course Configuration</CardTitle>
        <CardDescription>
          Configure how RAG works for this course
        </CardDescription>
      </CardHeader>
      <CardContent>
        {readOnly && (
          <p className="text-sm text-muted-foreground mb-4">
            Read-only: TAs can view but not edit course configuration.
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
            <Label htmlFor="name">Course Name</Label>
            <Input
              id="name"
              value={name}
              onChange={(e) => setName(e.target.value)}
            />
          </div>

          <div className="space-y-2">
            <Label htmlFor="description">Description</Label>
            <Textarea
              id="description"
              value={description}
              onChange={(e) => setDescription(e.target.value)}
            />
          </div>

          <Separator />

          <div className="space-y-2">
            <Label>
              RAG Context Ratio: {Math.round(contextRatio * 100)}%
            </Label>
            <Slider
              value={[contextRatio]}
              onValueChange={(v) => setContextRatio(Array.isArray(v) ? v[0] : v)}
              min={0.1}
              max={0.95}
              step={0.05}
            />
            <p className="text-xs text-muted-foreground">
              How much of the context window is used for RAG chunks vs
              conversation history
            </p>
          </div>

          <div className="space-y-2">
            <Label>Temperature: {temperature.toFixed(2)}</Label>
            <Slider
              value={[temperature]}
              onValueChange={(v) => setTemperature(Array.isArray(v) ? v[0] : v)}
              min={0}
              max={1}
              step={0.05}
            />
          </div>

          <div className="space-y-2">
            <Label>Model</Label>
            <Select value={model} onValueChange={(v) => v && setModel(v)}>
              <SelectTrigger className="w-full truncate">
                <SelectValue placeholder="Select a model" />
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
            <Label>Generation Strategy</Label>
            <Select value={strategy} onValueChange={(v) => v && setStrategy(v)}>
              <SelectTrigger className="w-full truncate">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="parallel">
                  Parallel (fast first token, inject RAG mid-stream)
                </SelectItem>
                <SelectItem value="simple">
                  Simple (retrieve first, then generate)
                </SelectItem>
                <SelectItem value="flare">
                  FLARE (sentence-level active retrieval)
                </SelectItem>
              </SelectContent>
            </Select>
            <p className="text-xs text-muted-foreground">
              Parallel starts streaming instantly and injects RAG context when ready.
              Simple waits for RAG before generating. FLARE retrieves after each sentence
              using the generated text as the query.
            </p>
          </div>

          <div className="space-y-2">
            <Label>Embedding Provider</Label>
            <Select value={embeddingProvider} onValueChange={(v) => v && setEmbeddingProvider(v)}>
              <SelectTrigger className="w-full truncate">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="local">
                  Local (FastEmbedding)
                </SelectItem>
                <SelectItem value="openai">
                  OpenAI (text-embedding-3-small)
                </SelectItem>
              </SelectContent>
            </Select>
            <p className="text-xs text-muted-foreground">
              Local runs on this server with zero latency and no API cost.
              OpenAI requires an API key. Changing provider requires re-uploading documents.
            </p>
          </div>

          {embeddingProvider === "local" && (
            <div className="space-y-2">
              <Label>Embedding Model</Label>
              <Select value={embeddingModel} onValueChange={(v) => v && setEmbeddingModel(v)}>
                <SelectTrigger className="w-full truncate">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  {[
                    { id: "sentence-transformers/all-MiniLM-L6-v2", name: "all-MiniLM-L6-v2", dims: 384, desc: "fast, good quality" },
                    { id: "BAAI/bge-small-en-v1.5", name: "BGE Small EN v1.5", dims: 384, desc: "optimized for retrieval" },
                    { id: "BAAI/bge-base-en-v1.5", name: "BGE Base EN v1.5", dims: 768, desc: "higher quality" },
                    { id: "nomic-ai/nomic-embed-text-v1.5", name: "Nomic Embed Text v1.5", dims: 768, desc: "long context" },
                  ].map((m) => {
                    const bench = benchmarksData?.benchmarks.find((b) => b.model === m.id)
                    const speed = bench ? ` - ${Math.round(bench.embeddings_per_second)} emb/s` : ""
                    return (
                      <SelectItem key={m.id} value={m.id}>
                        {m.name} ({m.dims}d, {m.desc}{speed})
                      </SelectItem>
                    )
                  })}
                </SelectContent>
              </Select>
              <p className="text-xs text-muted-foreground">
                Changing model requires re-uploading documents.
                {benchmarksData?.benchmarks.length ? " Speed measured on this server at startup." : ""}
              </p>
            </div>
          )}

          <div className="space-y-2">
            <Label htmlFor="maxChunks">Max Retrieved Chunks</Label>
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
            <Label>Minimum Similarity Score: {minScore.toFixed(2)}</Label>
            <Slider
              value={[minScore]}
              onValueChange={(v) => setMinScore(Array.isArray(v) ? v[0] : v)}
              min={0}
              max={1}
              step={0.01}
            />
            <p className="text-xs text-muted-foreground">
              Chunks scoring below this threshold are dropped before being sent
              to the model. 0 disables the filter (top-K only). Use the RAG tab
              to preview which chunks pass for a sample question.
            </p>
          </div>

          <Separator />

          <div className="space-y-2">
            <Label htmlFor="dailyTokenLimit">Daily Token Limit per Student</Label>
            <Input
              id="dailyTokenLimit"
              type="number"
              value={dailyTokenLimit}
              onChange={(e) => setDailyTokenLimit(parseInt(e.target.value) || 0)}
              min={0}
            />
            <p className="text-xs text-muted-foreground">
              Maximum tokens a student can use per day in this course. Set to 0 for unlimited.
            </p>
          </div>

          <Separator />

          <div className="space-y-2">
            <Label htmlFor="systemPrompt">Custom System Prompt</Label>
            <Textarea
              id="systemPrompt"
              value={systemPrompt}
              onChange={(e) => setSystemPrompt(e.target.value)}
              placeholder="Optional custom instructions for the AI assistant"
              rows={4}
            />
          </div>

          {!readOnly && (
            <Button type="submit" disabled={mutation.isPending}>
              {mutation.isPending ? "Saving..." : "Save Configuration"}
            </Button>
          )}
          {mutation.isSuccess && (
            <span className="text-sm text-muted-foreground ml-2">Saved!</span>
          )}
          {mutation.isError && (
            <p className="text-sm text-destructive">{mutation.error.message}</p>
          )}
        </form>
      </CardContent>
    </Card>
  )
}
