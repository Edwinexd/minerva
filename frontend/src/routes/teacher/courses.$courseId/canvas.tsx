import { createFileRoute } from "@tanstack/react-router"
import { RelativeTime } from "@/components/relative-time"
import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query"
import { canvasConnectionsQuery, canvasFilesQuery } from "@/lib/queries"
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
      setCoursesError(e instanceof Error ? e.message : "Failed to load courses")
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
        <p className="font-semibold text-amber-900 dark:text-amber-200">What Canvas sync does</p>
        <ul className="mt-2 list-disc space-y-1 pl-5 text-amber-900/90 dark:text-amber-200/90">
          <li>Pulls course files and page content only (no submissions, rosters, or grades).</li>
          <li>Your Canvas API token is stored in the Minerva database in plaintext. Revoke it in Minerva or Canvas to disconnect.</li>
          <li>Document content is indexed locally in Minerva and excerpts are sent to Cerebras when students ask related questions.</li>
        </ul>
      </div>
      <Card>
        <CardHeader>
          <CardTitle>Canvas Connections</CardTitle>
          <CardDescription>
            Connect a Canvas LMS course to sync its files into Minerva as documents.
            You'll need a Canvas personal access token with course file access.
          </CardDescription>
        </CardHeader>
        <CardContent className="space-y-4">
          {!showForm && (
            <Button onClick={() => setShowForm(true)}>Add Canvas Connection</Button>
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
                In Canvas, go to Account -&gt; Settings -&gt; New Access Token to generate an API token.
              </p>
              <div className="space-y-2">
                <Label htmlFor="canvas-name">Connection Name</Label>
                <Input id="canvas-name" value={name} onChange={(e) => setName(e.target.value)} placeholder="e.g. IK1203 HT2025" />
              </div>
              <div className="space-y-2">
                <Label htmlFor="canvas-url">Canvas URL</Label>
                <Input
                  id="canvas-url"
                  value={baseUrl}
                  onChange={(e) => { setBaseUrl(e.target.value); setAvailableCourses(null); setCoursesError(null) }}
                  placeholder="https://canvas.instructure.com"
                />
              </div>
              <div className="space-y-2">
                <Label htmlFor="canvas-token">API Token</Label>
                <Input
                  id="canvas-token"
                  type="password"
                  value={apiToken}
                  onChange={(e) => { setApiToken(e.target.value); setAvailableCourses(null); setCoursesError(null) }}
                  placeholder="Canvas personal access token"
                />
              </div>
              <div className="space-y-2">
                <div className="flex items-center justify-between">
                  <Label htmlFor="canvas-course-id">Canvas Course ID</Label>
                  {baseUrl.trim() && apiToken.trim() && (
                    <Button
                      type="button"
                      variant="ghost"
                      size="sm"
                      className="h-auto py-0 text-xs"
                      onClick={loadCourses}
                      disabled={isLoadingCourses || createMutation.isPending}
                    >
                      {isLoadingCourses ? "Loading..." : "Load courses"}
                    </Button>
                  )}
                </div>
                {availableCourses && availableCourses.length > 0 ? (
                  <Select value={canvasCourseId} onValueChange={(v) => v && setCanvasCourseId(v)}>
                    <SelectTrigger className="w-full">
                      <SelectValue placeholder="Select a Canvas course" />
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
                      placeholder="e.g. 12345"
                    />
                    {coursesError ? (
                      <p className="text-xs text-destructive">{coursesError}</p>
                    ) : (
                      <p className="text-xs text-muted-foreground">
                        Found in the Canvas course URL: canvas.example.com/courses/<strong>12345</strong>
                      </p>
                    )}
                  </>
                )}
              </div>

              {createMutation.isError && (
                <p className="text-sm text-destructive">{createMutation.error.message}</p>
              )}

              <div className="flex gap-2">
                <Button type="submit" disabled={createMutation.isPending || !baseUrl.trim() || !apiToken.trim() || !canvasCourseId.trim()}>
                  {createMutation.isPending ? "Connecting..." : "Save Connection"}
                </Button>
                <Button type="button" variant="outline" onClick={() => { setShowForm(false); setAvailableCourses(null); setCoursesError(null) }}>
                  Cancel
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
              No Canvas connections yet. Add one to start syncing course files.
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
                      <Badge variant="secondary">Course {conn.canvas_course_id}</Badge>
                    </div>
                    <div className="text-xs text-muted-foreground break-all">{conn.canvas_base_url}</div>
                    {conn.last_synced_at && (
                      <div className="text-xs text-muted-foreground">
                        Last synced: <RelativeTime date={conn.last_synced_at} />
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
                      <span>Auto-sync daily</span>
                    </label>
                  </div>
                  <div className="flex flex-wrap gap-2 sm:shrink-0">
                    <Button
                      variant="outline"
                      size="sm"
                      onClick={() => setSelectedConn(selectedConn === conn.id ? null : conn.id)}
                    >
                      Files
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
                      {syncMutation.isPending ? "Syncing..." : "Sync Now"}
                    </Button>
                    <Button
                      variant="destructive"
                      size="sm"
                      onClick={() => deleteMutation.mutate(conn.id)}
                      disabled={deleteMutation.isPending}
                    >
                      Remove
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
                <strong>Sync complete:</strong> {syncResult.synced} new, {syncResult.resynced} updated, {syncResult.skipped} unchanged
              </p>
              {syncResult.warnings.length > 0 && (
                <div className="text-amber-600 dark:text-amber-400">
                  <p>Warnings:</p>
                  <ul className="list-disc list-inside">
                    {syncResult.warnings.map((w, i) => <li key={i}>{w}</li>)}
                  </ul>
                </div>
              )}
              {syncResult.errors.length > 0 && (
                <div className="text-destructive">
                  <p>Errors:</p>
                  <ul className="list-disc list-inside">
                    {syncResult.errors.map((err, i) => <li key={i}>{err}</li>)}
                  </ul>
                </div>
              )}
            </div>
          )}

          {syncMutation.isError && (
            <p className="text-sm text-destructive">{syncMutation.error.message}</p>
          )}
        </CardContent>
      </Card>
    </div>
  )
}

function kindLabel(kind: string): string {
  if (kind === "file") return "File"
  if (kind === "page") return "Page"
  if (kind === "url") return "URL"
  return kind
}

function CanvasFilesPreview({ courseId, connectionId }: { courseId: string; connectionId: string }) {
  const { data, isLoading } = useQuery(canvasFilesQuery(courseId, connectionId))

  if (isLoading) return <Skeleton className="h-20 w-full" />
  if (!data) return null

  const { items, warnings } = data

  return (
    <div className="space-y-2">
      {warnings.length > 0 && (
        <div className="rounded border border-amber-500/40 bg-amber-50 dark:bg-amber-950/30 p-2 text-xs text-amber-700 dark:text-amber-300 space-y-1">
          {warnings.map((w, i) => <div key={i}>{w}</div>)}
        </div>
      )}
      {items.length === 0 ? (
        <p className="text-xs text-muted-foreground">No items found in this Canvas course.</p>
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
                <Badge variant="default" className="text-xs shrink-0">Update</Badge>
              ) : f.already_synced ? (
                <Badge variant="secondary" className="text-xs shrink-0">Synced</Badge>
              ) : (
                <Badge variant="outline" className="text-xs shrink-0">New</Badge>
              )}
            </div>
          ))}
        </div>
      )}
    </div>
  )
}
