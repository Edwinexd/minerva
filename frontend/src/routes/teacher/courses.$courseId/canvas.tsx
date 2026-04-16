import { createFileRoute } from "@tanstack/react-router"
import { RelativeTime } from "@/components/relative-time"
import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query"
import { useTranslation } from "react-i18next"
import { canvasConnectionsQuery, canvasFilesQuery } from "@/lib/queries"
import { api } from "@/lib/api"
import { useApiErrorMessage, useLocalizedMessage } from "@/lib/use-api-error"
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
import { Badge } from "@/components/ui/badge"
import { Checkbox } from "@/components/ui/checkbox"
import { Skeleton } from "@/components/ui/skeleton"
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select"
import { useState } from "react"
import type { CanvasConnection, CanvasSyncResult } from "@/lib/types"

type CanvasCourseInfo = { id: string; name: string; course_code: string | null }

export const Route = createFileRoute("/teacher/courses/$courseId/canvas")({
  component: CanvasPage,
})

function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`
}

function CanvasPage() {
  const { courseId } = Route.useParams()
  const queryClient = useQueryClient()
  const { t } = useTranslation("teacher")
  const { t: tCommon } = useTranslation("common")
  const formatError = useApiErrorMessage()
  const fmtMsg = useLocalizedMessage()
  const { data: connections, isLoading } = useQuery(canvasConnectionsQuery(courseId))
  const [showForm, setShowForm] = useState(false)
  const [name, setName] = useState("")
  const [baseUrl, setBaseUrl] = useState("")
  const [apiToken, setApiToken] = useState("")
  const [canvasCourseId, setCanvasCourseId] = useState("")
  const [selectedConn, setSelectedConn] = useState<string | null>(null)
  const [syncResult, setSyncResult] = useState<CanvasSyncResult | null>(null)
  const [availableCourses, setAvailableCourses] = useState<CanvasCourseInfo[] | null>(null)
  const [isLoadingCourses, setIsLoadingCourses] = useState(false)
  const [coursesError, setCoursesError] = useState<string | null>(null)

  const createMutation = useMutation({
    mutationFn: (data: {
      name: string
      canvas_base_url: string
      canvas_api_token: string
      canvas_course_id: string
    }) => api.post<CanvasConnection>(`/courses/${courseId}/canvas`, data),
    onSuccess: () => {
      setShowForm(false)
      setName("")
      setBaseUrl("")
      setApiToken("")
      setCanvasCourseId("")
      setAvailableCourses(null)
      setCoursesError(null)
      queryClient.invalidateQueries({
        queryKey: ["courses", courseId, "canvas"],
      })
    },
  })

  const loadCourses = async () => {
    setIsLoadingCourses(true)
    setCoursesError(null)
    try {
      const courses = await api.post<CanvasCourseInfo[]>(
        `/courses/${courseId}/canvas/lookup-courses`,
        { canvas_base_url: baseUrl.trim(), canvas_api_token: apiToken.trim() },
      )
      setAvailableCourses(courses)
      if (courses.length > 0 && !canvasCourseId) {
        setCanvasCourseId(courses[0].id)
      }
    } catch (e) {
      setCoursesError(e instanceof Error ? formatError(e) : t("canvas.loadCoursesFailed"))
      setAvailableCourses(null)
    } finally {
      setIsLoadingCourses(false)
    }
  }

  const deleteMutation = useMutation({
    mutationFn: (connId: string) =>
      api.delete(`/courses/${courseId}/canvas/${connId}`),
    onSuccess: () => {
      queryClient.invalidateQueries({
        queryKey: ["courses", courseId, "canvas"],
      })
    },
  })

  const syncMutation = useMutation({
    mutationFn: (connId: string) =>
      api.post<CanvasSyncResult>(`/courses/${courseId}/canvas/${connId}/sync`, {}),
    onSuccess: (data) => {
      setSyncResult(data)
      queryClient.invalidateQueries({
        queryKey: ["courses", courseId, "canvas"],
      })
      queryClient.invalidateQueries({
        queryKey: ["courses", courseId, "documents"],
      })
    },
  })

  const autoSyncMutation = useMutation({
    mutationFn: ({ connId, autoSync }: { connId: string; autoSync: boolean }) =>
      api.patch<CanvasConnection>(
        `/courses/${courseId}/canvas/${connId}/auto-sync`,
        { auto_sync: autoSync },
      ),
    onSuccess: () => {
      queryClient.invalidateQueries({
        queryKey: ["courses", courseId, "canvas"],
      })
    },
  })

  return (
    <div className="space-y-4">
      <div className="rounded-md border border-amber-300 bg-amber-50 px-4 py-3 text-sm dark:border-amber-800 dark:bg-amber-950/40">
        <p className="font-semibold text-amber-900 dark:text-amber-200">{t("canvas.noticeTitle")}</p>
        <ul className="mt-2 list-disc space-y-1 pl-5 text-amber-900/90 dark:text-amber-200/90">
          <li>{t("canvas.noticeBullet1")}</li>
          <li>{t("canvas.noticeBullet2")}</li>
          <li>{t("canvas.noticeBullet3")}</li>
        </ul>
      </div>
      <Card>
        <CardHeader>
          <CardTitle>{t("canvas.connectionsTitle")}</CardTitle>
          <CardDescription>
            {t("canvas.connectionsDescription")}
          </CardDescription>
        </CardHeader>
        <CardContent className="space-y-4">
          {!showForm && (
            <Button onClick={() => setShowForm(true)}>{t("canvas.addConnection")}</Button>
          )}

          {showForm && (
            <form
              className="space-y-3 rounded-md border p-4"
              onSubmit={(e) => {
                e.preventDefault()
                createMutation.mutate({
                  name: name.trim(),
                  canvas_base_url: baseUrl.trim(),
                  canvas_api_token: apiToken.trim(),
                  canvas_course_id: canvasCourseId.trim(),
                })
              }}
            >
              <p className="text-sm text-muted-foreground">
                {t("canvas.formTokenHint")}
              </p>
              <div className="space-y-2">
                <Label htmlFor="canvas-name">{t("canvas.connectionNameLabel")}</Label>
                <Input id="canvas-name" value={name} onChange={(e) => setName(e.target.value)} placeholder={t("canvas.connectionNamePlaceholder")} />
              </div>
              <div className="space-y-2">
                <Label htmlFor="canvas-url">{t("canvas.baseUrlLabel")}</Label>
                <Input
                  id="canvas-url"
                  value={baseUrl}
                  onChange={(e) => { setBaseUrl(e.target.value); setAvailableCourses(null); setCoursesError(null) }}
                  placeholder={t("canvas.baseUrlPlaceholder")}
                />
              </div>
              <div className="space-y-2">
                <Label htmlFor="canvas-token">{t("canvas.tokenLabel")}</Label>
                <Input
                  id="canvas-token"
                  type="password"
                  value={apiToken}
                  onChange={(e) => { setApiToken(e.target.value); setAvailableCourses(null); setCoursesError(null) }}
                  placeholder={t("canvas.tokenPlaceholder")}
                />
              </div>
              <div className="space-y-2">
                <div className="flex items-center justify-between">
                  <Label htmlFor="canvas-course-id">{t("canvas.courseIdLabel")}</Label>
                  {baseUrl.trim() && apiToken.trim() && (
                    <Button
                      type="button"
                      variant="ghost"
                      size="sm"
                      className="h-auto py-0 text-xs"
                      onClick={loadCourses}
                      disabled={isLoadingCourses || createMutation.isPending}
                    >
                      {isLoadingCourses ? t("canvas.loadingCourses") : t("canvas.loadCourses")}
                    </Button>
                  )}
                </div>
                {availableCourses && availableCourses.length > 0 ? (
                  <Select value={canvasCourseId} onValueChange={(v) => v && setCanvasCourseId(v)}>
                    <SelectTrigger className="w-full">
                      <SelectValue placeholder={t("canvas.selectCoursePlaceholder")} />
                    </SelectTrigger>
                    <SelectContent>
                      {availableCourses.map((c) => (
                        <SelectItem key={c.id} value={c.id}>
                          {c.name}{c.course_code ? ` (${c.course_code})` : ""} ({c.id})
                        </SelectItem>
                      ))}
                    </SelectContent>
                  </Select>
                ) : (
                  <>
                    <Input
                      id="canvas-course-id"
                      value={canvasCourseId}
                      onChange={(e) => setCanvasCourseId(e.target.value)}
                      placeholder={t("canvas.courseIdPlaceholder")}
                    />
                    {coursesError ? (
                      <p className="text-xs text-destructive">{coursesError}</p>
                    ) : (
                      <p className="text-xs text-muted-foreground">
                        {t("canvas.courseIdHelpPrefix")}<strong>{t("canvas.courseIdHelpSample")}</strong>
                      </p>
                    )}
                  </>
                )}
              </div>

              {createMutation.isError && (
                <p className="text-sm text-destructive">{formatError(createMutation.error)}</p>
              )}

              <div className="flex gap-2">
                <Button type="submit" disabled={createMutation.isPending || !baseUrl.trim() || !apiToken.trim() || !canvasCourseId.trim()}>
                  {createMutation.isPending ? t("canvas.connectingButton") : t("canvas.saveConnection")}
                </Button>
                <Button type="button" variant="outline" onClick={() => { setShowForm(false); setAvailableCourses(null); setCoursesError(null) }}>
                  {tCommon("actions.cancel")}
                </Button>
              </div>
            </form>
          )}

          {isLoading && (
            <div className="space-y-2">
              <Skeleton className="h-10 w-full" />
            </div>
          )}

          {connections && connections.length === 0 && !showForm && (
            <p className="text-sm text-muted-foreground py-4 text-center">
              {t("canvas.noConnections")}
            </p>
          )}

          <div className="space-y-3">
            {connections?.map((conn) => (
              <div
                key={conn.id}
                className="space-y-3 py-3 border-b last:border-0"
              >
                <div className="flex flex-col gap-3 sm:flex-row sm:items-start sm:justify-between">
                  <div className="space-y-1 min-w-0 sm:flex-1">
                    <div className="flex flex-wrap items-center gap-2">
                      <span className="font-medium text-sm">{conn.name}</span>
                      <Badge variant="secondary">{t("canvas.courseBadge", { id: conn.canvas_course_id })}</Badge>
                    </div>
                    <div className="text-xs text-muted-foreground break-all">{conn.canvas_base_url}</div>
                    {conn.last_synced_at && (
                      <div className="text-xs text-muted-foreground">
                        {t("canvas.lastSynced")} <RelativeTime date={conn.last_synced_at} />
                      </div>
                    )}
                    <label className="flex items-center gap-2 text-xs text-muted-foreground pt-1 cursor-pointer select-none">
                      <Checkbox
                        checked={conn.auto_sync}
                        onCheckedChange={(checked) =>
                          autoSyncMutation.mutate({
                            connId: conn.id,
                            autoSync: checked === true,
                          })
                        }
                        disabled={autoSyncMutation.isPending}
                      />
                      <span>{t("canvas.autoSync")}</span>
                    </label>
                  </div>
                  <div className="flex flex-wrap gap-2 sm:shrink-0">
                    <Button
                      variant="outline"
                      size="sm"
                      onClick={() => setSelectedConn(selectedConn === conn.id ? null : conn.id)}
                    >
                      {t("canvas.filesButton")}
                    </Button>
                    <Button
                      variant="default"
                      size="sm"
                      onClick={() => {
                        setSyncResult(null)
                        syncMutation.mutate(conn.id)
                      }}
                      disabled={syncMutation.isPending}
                    >
                      {syncMutation.isPending ? t("canvas.syncingButton") : t("canvas.syncNow")}
                    </Button>
                    <Button
                      variant="destructive"
                      size="sm"
                      onClick={() => deleteMutation.mutate(conn.id)}
                      disabled={deleteMutation.isPending}
                    >
                      {t("canvas.remove")}
                    </Button>
                  </div>
                </div>

                {selectedConn === conn.id && (
                  <CanvasFilesPreview courseId={courseId} connectionId={conn.id} />
                )}
              </div>
            ))}
          </div>

          {syncResult && (
            <div className="rounded-md border p-3 text-sm space-y-1">
              <p>
                <strong>{t("canvas.syncComplete")}</strong> {t("canvas.syncCounts", { synced: syncResult.synced, resynced: syncResult.resynced, skipped: syncResult.skipped })}
              </p>
              {syncResult.warnings.length > 0 && (
                <div className="text-amber-600 dark:text-amber-400">
                  <p>{t("canvas.warningsLabel")}</p>
                  <ul className="list-disc list-inside">
                    {syncResult.warnings.map((w, i) => <li key={i}>{fmtMsg(w)}</li>)}
                  </ul>
                </div>
              )}
              {syncResult.errors.length > 0 && (
                <div className="text-destructive">
                  <p>{t("canvas.errorsLabel")}</p>
                  <ul className="list-disc list-inside">
                    {syncResult.errors.map((err, i) => <li key={i}>{fmtMsg(err)}</li>)}
                  </ul>
                </div>
              )}
            </div>
          )}

          {syncMutation.isError && (
            <p className="text-sm text-destructive">{formatError(syncMutation.error)}</p>
          )}
        </CardContent>
      </Card>
    </div>
  )
}

function CanvasFilesPreview({ courseId, connectionId }: { courseId: string; connectionId: string }) {
  const { data, isLoading } = useQuery(canvasFilesQuery(courseId, connectionId))
  const { t } = useTranslation("teacher")
  const fmtMsg = useLocalizedMessage()

  const kindLabel = (kind: string): string => {
    if (kind === "file") return t("canvas.kindFile")
    if (kind === "page") return t("canvas.kindPage")
    if (kind === "url") return t("canvas.kindUrl")
    return kind
  }

  if (isLoading) return <Skeleton className="h-20 w-full" />
  if (!data) return null

  const { items, warnings } = data

  return (
    <div className="space-y-2">
      {warnings.length > 0 && (
        <div className="rounded border border-amber-500/40 bg-amber-50 dark:bg-amber-950/30 p-2 text-xs text-amber-700 dark:text-amber-300 space-y-1">
          {warnings.map((w, i) => <div key={i}>{fmtMsg(w)}</div>)}
        </div>
      )}
      {items.length === 0 ? (
        <p className="text-xs text-muted-foreground">{t("canvas.filesNoItems")}</p>
      ) : (
        <div className="rounded border p-2 text-xs max-h-48 overflow-y-auto space-y-1">
          {items.map((f) => (
            <div key={f.id} className="flex items-center justify-between gap-2 py-0.5">
              <Badge variant="outline" className="text-[10px] shrink-0">{kindLabel(f.kind)}</Badge>
              <span className="truncate flex-1">{f.filename}</span>
              {f.size > 0 && (
                <span className="text-muted-foreground shrink-0">{formatBytes(f.size)}</span>
              )}
              {f.needs_resync ? (
                <Badge variant="default" className="text-xs shrink-0">{t("canvas.statusUpdate")}</Badge>
              ) : f.already_synced ? (
                <Badge variant="secondary" className="text-xs shrink-0">{t("canvas.statusSynced")}</Badge>
              ) : (
                <Badge variant="outline" className="text-xs shrink-0">{t("canvas.statusNew")}</Badge>
              )}
            </div>
          ))}
        </div>
      )}
    </div>
  )
}
