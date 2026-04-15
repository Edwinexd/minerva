import { createFileRoute } from "@tanstack/react-router"
import { RelativeTime } from "@/components/relative-time"
import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query"
import { playCourseCatalogQuery, playDesignationsQuery } from "@/lib/queries"
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
import { Skeleton } from "@/components/ui/skeleton"
import { useState } from "react"
import type { PlayDesignation } from "@/lib/types"

export const Route = createFileRoute(
  "/teacher/courses/$courseId/play-designations",
)({
  component: PlayDesignationsPage,
})

function PlayDesignationsPage() {
  const { courseId } = Route.useParams()
  const queryClient = useQueryClient()
  const [designation, setDesignation] = useState("")
  const { data: designations, isLoading } = useQuery(
    playDesignationsQuery(courseId),
  )
  const { data: catalog } = useQuery(playCourseCatalogQuery)

  const createMutation = useMutation({
    mutationFn: (data: { designation: string }) =>
      api.post<PlayDesignation>(
        `/courses/${courseId}/play-designations`,
        data,
      ),
    onSuccess: () => {
      setDesignation("")
      queryClient.invalidateQueries({
        queryKey: ["courses", courseId, "play-designations"],
      })
    },
  })

  const deleteMutation = useMutation({
    mutationFn: (id: string) =>
      api.delete(`/courses/${courseId}/play-designations/${id}`),
    onSuccess: () => {
      queryClient.invalidateQueries({
        queryKey: ["courses", courseId, "play-designations"],
      })
    },
  })

  return (
    <Card>
      <CardHeader>
        <CardTitle>Play Designations</CardTitle>
        <CardDescription>
          Watch course designations on play.dsv.su.se (e.g. <code>PROG1</code>,{" "}
          <code>IDSV</code>). The hourly transcript pipeline will discover any
          new presentations under each designation and auto-index their
          transcripts into this course.
        </CardDescription>
      </CardHeader>
      <CardContent className="space-y-4">
        <form
          className="flex gap-2"
          onSubmit={(e) => {
            e.preventDefault()
            const trimmed = designation.trim()
            if (trimmed) {
              createMutation.mutate({ designation: trimmed })
            }
          }}
        >
          <Input
            value={designation}
            onChange={(e) => setDesignation(e.target.value)}
            placeholder="Designation code (e.g. PROG1)"
            className="flex-1"
            list="play-course-catalog"
            autoCapitalize="characters"
          />
          <datalist id="play-course-catalog">
            {catalog?.map((c) => (
              <option key={c.code} value={c.code}>
                {c.name}
              </option>
            ))}
          </datalist>
          <Button
            type="submit"
            disabled={createMutation.isPending || !designation.trim()}
          >
            {createMutation.isPending ? "Adding..." : "Add"}
          </Button>
        </form>

        {createMutation.isError && (
          <p className="text-sm text-destructive">
            {createMutation.error.message}
          </p>
        )}

        {isLoading && (
          <div className="space-y-2">
            <Skeleton className="h-10 w-full" />
            <Skeleton className="h-10 w-full" />
          </div>
        )}

        {designations && designations.length === 0 && (
          <p className="text-sm text-muted-foreground py-4 text-center">
            No designations watched yet. Add one to auto-index presentations.
          </p>
        )}

        <div className="space-y-3">
          {designations?.map((d) => (
            <div
              key={d.id}
              className="flex items-center justify-between py-2 border-b last:border-0"
            >
              <div className="space-y-1 flex-1 min-w-0">
                <div className="flex items-center gap-2">
                  <code className="font-mono text-sm bg-muted px-1.5 py-0.5 rounded">
                    {d.designation}
                  </code>
                </div>
                <div className="flex gap-3 text-xs text-muted-foreground">
                  <span>
                    Added: <RelativeTime date={d.created_at} />
                  </span>
                  <span>
                    Last sync:{" "}
                    {d.last_synced_at ? <RelativeTime date={d.last_synced_at} /> : "never"}
                  </span>
                </div>
                {d.last_error && (
                  <p className="text-xs text-destructive">
                    Last error: {d.last_error}
                  </p>
                )}
              </div>
              <Button
                variant="destructive"
                size="sm"
                onClick={() => deleteMutation.mutate(d.id)}
                disabled={deleteMutation.isPending}
              >
                Remove
              </Button>
            </div>
          ))}
        </div>
      </CardContent>
    </Card>
  )
}
