import { createFileRoute } from "@tanstack/react-router"
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
import { Skeleton } from "@/components/ui/skeleton"
import { useState } from "react"
import type { CanvasConnection, CanvasSyncResult } from "@/lib/types"

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
      queryClient.invalidateQueries({
        queryKey: ["courses", courseId, "canvas"],
      })
    },
  })

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

  return (
    <div className="space-y-4">
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
                <Input id="canvas-url" value={baseUrl} onChange={(e) => setBaseUrl(e.target.value)} placeholder="https://canvas.instructure.com" />
              </div>
              <div className="space-y-2">
                <Label htmlFor="canvas-token">API Token</Label>
                <Input id="canvas-token" type="password" value={apiToken} onChange={(e) => setApiToken(e.target.value)} placeholder="Canvas personal access token" />
              </div>
              <div className="space-y-2">
                <Label htmlFor="canvas-course-id">Canvas Course ID</Label>
                <Input id="canvas-course-id" value={canvasCourseId} onChange={(e) => setCanvasCourseId(e.target.value)} placeholder="e.g. 12345" />
                <p className="text-xs text-muted-foreground">
                  Found in the Canvas course URL: canvas.example.com/courses/<strong>12345</strong>
                </p>
              </div>

              {createMutation.isError && (
                <p className="text-sm text-destructive">{createMutation.error.message}</p>
              )}

              <div className="flex gap-2">
                <Button type="submit" disabled={createMutation.isPending || !baseUrl.trim() || !apiToken.trim() || !canvasCourseId.trim()}>
                  {createMutation.isPending ? "Connecting..." : "Save Connection"}
                </Button>
                <Button type="button" variant="outline" onClick={() => setShowForm(false)}>
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
                className="space-y-2 py-3 border-b last:border-0"
              >
                <div className="flex items-center justify-between gap-2">
                  <div className="space-y-1 flex-1 min-w-0">
                    <div className="flex items-center gap-2">
                      <span className="font-medium text-sm">{conn.name}</span>
                      <Badge variant="secondary">Course {conn.canvas_course_id}</Badge>
                    </div>
                    <div className="text-xs text-muted-foreground truncate">{conn.canvas_base_url}</div>
                    {conn.last_synced_at && (
                      <div className="text-xs text-muted-foreground">
                        Last synced: {new Date(conn.last_synced_at).toLocaleString()}
                      </div>
                    )}
                  </div>
                  <div className="flex gap-2 shrink-0">
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
              <p><strong>Sync complete:</strong> {syncResult.synced} files synced, {syncResult.skipped} skipped</p>
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

function CanvasFilesPreview({ courseId, connectionId }: { courseId: string; connectionId: string }) {
  const { data: files, isLoading } = useQuery(canvasFilesQuery(courseId, connectionId))

  if (isLoading) return <Skeleton className="h-20 w-full" />
  if (!files || files.length === 0) return <p className="text-xs text-muted-foreground">No files found in this Canvas course.</p>

  return (
    <div className="rounded border p-2 text-xs max-h-48 overflow-y-auto space-y-1">
      {files.map((f) => (
        <div key={f.id} className="flex items-center justify-between gap-2 py-0.5">
          <span className="truncate flex-1">{f.filename}</span>
          <span className="text-muted-foreground shrink-0">{formatBytes(f.size)}</span>
          {f.already_synced ? (
            <Badge variant="secondary" className="text-xs shrink-0">Synced</Badge>
          ) : (
            <Badge variant="outline" className="text-xs shrink-0">New</Badge>
          )}
        </div>
      ))}
    </div>
  )
}
