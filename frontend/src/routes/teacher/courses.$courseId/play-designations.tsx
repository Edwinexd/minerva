import { createFileRoute } from "@tanstack/react-router"
import { RelativeTime } from "@/components/relative-time"
import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query"
import { useTranslation } from "react-i18next"
import { playCourseCatalogQuery, playDesignationsQuery } from "@/lib/queries"
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
  const { t } = useTranslation("teacher")
  const formatError = useApiErrorMessage()
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
        <CardTitle>{t("playDesignations.title")}</CardTitle>
        <CardDescription>
          {t("playDesignations.descriptionPrefix")}<code>PROG1</code>{t("playDesignations.descriptionMid")}
          <code>IDSV</code>{t("playDesignations.descriptionSuffix")}
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
            placeholder={t("playDesignations.placeholder")}
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
            {createMutation.isPending ? t("playDesignations.adding") : t("playDesignations.add")}
          </Button>
        </form>

        {createMutation.isError && (
          <p className="text-sm text-destructive">
            {formatError(createMutation.error)}
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
            {t("playDesignations.empty")}
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
                    {t("playDesignations.added")} <RelativeTime date={d.created_at} />
                  </span>
                  <span>
                    {t("playDesignations.lastSync")}{" "}
                    {d.last_synced_at ? <RelativeTime date={d.last_synced_at} /> : t("playDesignations.never")}
                  </span>
                </div>
                {d.last_error && (
                  <p className="text-xs text-destructive">
                    {t("playDesignations.lastError", { message: d.last_error })}
                  </p>
                )}
              </div>
              <Button
                variant="destructive"
                size="sm"
                onClick={() => deleteMutation.mutate(d.id)}
                disabled={deleteMutation.isPending}
              >
                {t("playDesignations.remove")}
              </Button>
            </div>
          ))}
        </div>
      </CardContent>
    </Card>
  )
}
