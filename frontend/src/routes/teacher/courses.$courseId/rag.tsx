import { createFileRoute } from "@tanstack/react-router"
import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query"
import { useTranslation } from "react-i18next"
import { courseDocumentsQuery, courseQuery } from "@/lib/queries"
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
import { Badge } from "@/components/ui/badge"
import { Skeleton } from "@/components/ui/skeleton"
import { Slider } from "@/components/ui/slider"
import { Label } from "@/components/ui/label"
import { useState } from "react"
import type { Course } from "@/lib/types"

export const Route = createFileRoute("/teacher/courses/$courseId/rag")({
  component: RagPage,
})

function RagPage() {
  const { courseId } = Route.useParams()
  const { data: course } = useQuery(courseQuery(courseId))
  return (
    <div className="space-y-4">
      <RagDebugPanel courseId={courseId} course={course} />
      <ChunkBrowser courseId={courseId} />
    </div>
  )
}

function RagDebugPanel({
  courseId,
  course,
}: {
  courseId: string
  course?: Course
}) {
  const queryClient = useQueryClient()
  const { t } = useTranslation("teacher")
  const [query, setQuery] = useState("")
  const [results, setResults] = useState<
    { score: number; text: string; filename: string; chunk_index: number }[]
  >([])
  const [searching, setSearching] = useState(false)
  // Local preview threshold; defaults to the saved course value but can be
  // dragged around to see what cutoff WOULD do without saving. "Save"
  // persists the value back to the course config.
  const [previewThreshold, setPreviewThreshold] = useState<number | null>(null)
  const effectiveThreshold = previewThreshold ?? course?.min_score ?? 0
  const dirty =
    course != null &&
    previewThreshold != null &&
    Math.abs(previewThreshold - course.min_score) > 1e-6

  const saveMutation = useMutation({
    mutationFn: (min_score: number) =>
      api.put<Course>(`/courses/${courseId}`, { min_score }),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["courses", courseId] })
      queryClient.invalidateQueries({ queryKey: ["courses"] })
      setPreviewThreshold(null)
    },
  })

  const doSearch = async () => {
    if (!query.trim()) return
    setSearching(true)
    try {
      const res = await api.get<typeof results>(
        `/courses/${courseId}/documents/search?q=${encodeURIComponent(query)}&limit=10`,
      )
      setResults(res)
    } catch {
      setResults([])
    } finally {
      setSearching(false)
    }
  }

  const includedCount = results.filter((r) => r.score >= effectiveThreshold).length

  return (
    <Card>
      <CardHeader>
        <CardTitle>{t("rag.searchTitle")}</CardTitle>
        <CardDescription>
          {t("rag.searchDescription")}
        </CardDescription>
      </CardHeader>
      <CardContent className="space-y-4">
        <form
          className="flex gap-2"
          onSubmit={(e) => {
            e.preventDefault()
            doSearch()
          }}
        >
          <Input
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder={t("rag.queryPlaceholder")}
            className="flex-1"
          />
          <Button type="submit" disabled={searching || !query.trim()}>
            {searching ? t("rag.searching") : t("rag.search")}
          </Button>
        </form>

        {course && (
          <div className="space-y-2 rounded border p-3 bg-muted/30">
            <div className="flex items-center justify-between gap-3">
              <Label className="text-sm">
                {t("rag.thresholdLabel")}{" "}
                <span className="font-mono">{effectiveThreshold.toFixed(2)}</span>
                {dirty && (
                  <span className="ml-2 text-xs text-muted-foreground">
                    {t("rag.thresholdPreview", { value: course.min_score.toFixed(2) })}
                  </span>
                )}
              </Label>
              {results.length > 0 && (
                <span className="text-xs text-muted-foreground">
                  {t("rag.includedCount", { included: includedCount, total: results.length })}
                </span>
              )}
            </div>
            <Slider
              value={[effectiveThreshold]}
              onValueChange={(v) =>
                setPreviewThreshold(Array.isArray(v) ? v[0] : v)
              }
              min={0}
              max={1}
              step={0.01}
            />
            <div className="flex gap-2">
              <Button
                type="button"
                size="sm"
                disabled={!dirty || saveMutation.isPending}
                onClick={() => {
                  if (previewThreshold != null) saveMutation.mutate(previewThreshold)
                }}
              >
                {saveMutation.isPending ? t("rag.saving") : t("rag.saveThreshold")}
              </Button>
              {dirty && (
                <Button
                  type="button"
                  variant="ghost"
                  size="sm"
                  onClick={() => setPreviewThreshold(null)}
                  disabled={saveMutation.isPending}
                >
                  {t("rag.reset")}
                </Button>
              )}
            </div>
          </div>
        )}

        {results.length > 0 && (
          <div className="space-y-3">
            {results.map((r, i) => {
              const included = r.score >= effectiveThreshold
              return (
                <div
                  key={i}
                  className={`border rounded p-3 space-y-1 ${
                    included ? "" : "opacity-50 border-dashed"
                  }`}
                >
                  <div className="flex items-center justify-between gap-2">
                    <span className="text-sm font-medium truncate">{r.filename}</span>
                    <div className="flex items-center gap-2 shrink-0">
                      <Badge
                        variant={
                          included
                            ? r.score > 0.7
                              ? "default"
                              : "secondary"
                            : "outline"
                        }
                      >
                        {r.score.toFixed(3)}
                      </Badge>
                      <Badge variant={included ? "default" : "outline"}>
                        {included ? t("rag.included") : t("rag.excluded")}
                      </Badge>
                    </div>
                  </div>
                  <p className="text-xs text-muted-foreground">
                    {t("rag.chunkIndex", { index: r.chunk_index })}
                  </p>
                  <p className="text-sm whitespace-pre-wrap line-clamp-4">{r.text}</p>
                </div>
              )
            })}
          </div>
        )}
      </CardContent>
    </Card>
  )
}

function ChunkBrowser({ courseId }: { courseId: string }) {
  const { t } = useTranslation("teacher")
  const { data: documents } = useQuery(courseDocumentsQuery(courseId))
  const [selectedDoc, setSelectedDoc] = useState<string | null>(null)
  const { data: chunks, isLoading: chunksLoading } = useQuery({
    queryKey: ["courses", courseId, "documents", selectedDoc, "chunks"],
    queryFn: () =>
      api.get<{ chunk_index: number; text: string; filename: string }[]>(
        `/courses/${courseId}/documents/${selectedDoc}/chunks`,
      ),
    enabled: !!selectedDoc,
  })

  const readyDocs = documents?.filter((d) => d.status === "ready") || []

  return (
    <Card>
      <CardHeader>
        <CardTitle>{t("rag.browserTitle")}</CardTitle>
        <CardDescription>
          {t("rag.browserDescription")}
        </CardDescription>
      </CardHeader>
      <CardContent className="space-y-4">
        {readyDocs.length === 0 ? (
          <p className="text-muted-foreground text-sm">
            {t("rag.noProcessed")}
          </p>
        ) : (
          <div className="flex gap-2 flex-wrap">
            {readyDocs.map((doc) => (
              <Button
                key={doc.id}
                variant={selectedDoc === doc.id ? "default" : "outline"}
                size="sm"
                onClick={() => setSelectedDoc(doc.id)}
              >
                {t("rag.docButton", { filename: doc.filename, count: doc.chunk_count || 0 })}
              </Button>
            ))}
          </div>
        )}

        {chunksLoading && (
          <div className="space-y-2">
            <Skeleton className="h-20 w-full" />
            <Skeleton className="h-20 w-full" />
          </div>
        )}

        {chunks && (
          <div className="space-y-2 max-h-96 overflow-y-auto">
            {chunks.map((chunk) => (
              <div
                key={chunk.chunk_index}
                className="border rounded p-3 text-sm"
              >
                <div className="text-xs text-muted-foreground mb-1">
                  {t("rag.chunkIndex", { index: chunk.chunk_index })}
                </div>
                <p className="whitespace-pre-wrap">{chunk.text}</p>
              </div>
            ))}
          </div>
        )}
      </CardContent>
    </Card>
  )
}
