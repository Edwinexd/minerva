import { Link } from "@tanstack/react-router"
import { RelativeTime } from "@/components/relative-time"
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query"
import { useTranslation } from "react-i18next"
import { adminUsersQuery, coursesQuery } from "@/lib/queries"
import { api } from "@/lib/api"
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { Checkbox } from "@/components/ui/checkbox"
import {
  AlertDialog,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog"
import { Skeleton } from "@/components/ui/skeleton"
import { useState } from "react"
import { useApiErrorMessage } from "@/lib/use-api-error"

/// Single source of truth for known flags on the frontend, mirrored
/// from the backend's `feature_flags::ALL_FLAGS`. Adding a flag here
/// + i18n key + bumping the backend list is everything needed for
/// the admin UI to surface a new toggle.
const KNOWN_FEATURE_FLAGS = ["course_kg", "extraction_guard"] as const
type FeatureFlagName = (typeof KNOWN_FEATURE_FLAGS)[number]

export function CourseManagementPanel() {
  const { t } = useTranslation("admin")
  const { data: courses, isLoading: coursesLoading } = useQuery(coursesQuery)
  const { data: users } = useQuery(adminUsersQuery)
  const [filter, setFilter] = useState("")

  if (coursesLoading) {
    return (
      <div className="space-y-2">
        {Array.from({ length: 5 }).map((_, i) => (
          <Skeleton key={i} className="h-14 w-full" />
        ))}
      </div>
    )
  }

  if (!courses) return null

  const userMap = new Map((users ?? []).map((u) => [u.id, u]))

  const filtered = filter
    ? courses.filter((c) => {
        const owner = userMap.get(c.owner_id)
        const ownerLabel = owner?.display_name ?? owner?.eppn ?? c.owner_id
        return (
          c.name.toLowerCase().includes(filter.toLowerCase()) ||
          ownerLabel.toLowerCase().includes(filter.toLowerCase())
        )
      })
    : courses

  return (
    <Card>
      <CardHeader>
        <CardTitle>{t("courses.title", { total: courses.length })}</CardTitle>
        <CardDescription>{t("courses.description")}</CardDescription>
        <input
          className="mt-2 w-full max-w-sm rounded border bg-background px-3 py-1.5 text-sm"
          placeholder={t("courses.filterPlaceholder")}
          aria-label={t("courses.filterPlaceholder")}
          value={filter}
          onChange={(e) => setFilter(e.target.value)}
        />
      </CardHeader>
      <CardContent>
        <div className="overflow-x-auto">
          <table className="w-full text-sm">
            <thead>
              <tr className="border-b text-left">
                <th className="py-2 pr-4 font-medium">{t("courses.columns.course")}</th>
                <th className="py-2 pr-4 font-medium">{t("courses.columns.owner")}</th>
                <th className="py-2 pr-4 font-medium">{t("courses.columns.status")}</th>
                <th className="py-2 pr-4 font-medium">{t("courses.columns.tokenLimit")}</th>
                <th className="py-2 pr-4 font-medium">{t("courses.columns.created")}</th>
                <th className="py-2 pr-4 font-medium">{t("courses.columns.features")}</th>
                <th className="py-2 font-medium">{t("courses.columns.settings")}</th>
              </tr>
            </thead>
            <tbody>
              {filtered.map((course) => {
                const owner = userMap.get(course.owner_id)
                const ownerLabel =
                  owner?.display_name ?? owner?.eppn ?? course.owner_id.slice(0, 8)
                return (
                  <tr key={course.id} className="border-b">
                    <td className="py-2 pr-4 font-medium">{course.name}</td>
                    <td className="py-2 pr-4 text-muted-foreground">
                      {ownerLabel}
                    </td>
                    <td className="py-2 pr-4">
                      {course.active ? (
                        <Badge variant="secondary">{t("courses.status.active")}</Badge>
                      ) : (
                        <Badge variant="outline">{t("courses.status.archived")}</Badge>
                      )}
                    </td>
                    <td className="py-2 pr-4 font-mono">
                      {course.daily_token_limit === 0
                        ? t("courses.tokenLimitUnlimited")
                        : course.daily_token_limit.toLocaleString()}
                    </td>
                    <td className="py-2 pr-4 text-muted-foreground">
                      <RelativeTime date={course.created_at} />
                    </td>
                    <td className="py-2 pr-4">
                      <CourseFeatureFlagsCell
                        courseId={course.id}
                        courseName={course.name}
                      />
                    </td>
                    <td className="py-2">
                      <Link
                        to="/teacher/courses/$courseId/config"
                        params={{ courseId: course.id }}
                        className="text-primary underline-offset-4 hover:underline"
                      >
                        {t("courses.settingsLink")}
                      </Link>
                    </td>
                  </tr>
                )
              })}
            </tbody>
          </table>
          {filtered.length === 0 && (
            <p className="py-4 text-center text-sm text-muted-foreground">
              {t("courses.empty")}
            </p>
          )}
        </div>
      </CardContent>
    </Card>
  )
}

// ── Per-course feature flags ───────────────────────────────────────
//
// A multi-selector dialog: the cell shows a button summarising the
// current state ("2/3 features"); clicking opens an AlertDialog with
// one checkbox per known flag. Flipping a checkbox PUTs the new
// state immediately so the admin sees the effect without a separate
// "save" action.
//
// We re-fetch the per-course flag state from the dedicated admin
// endpoint when the dialog opens (rather than inferring it from the
// general courses list response): admins may have made changes since
// the courses list was cached, and the dedicated endpoint also tells
// us which flags are explicitly overridden vs inherited from global.

interface FeatureFlagState {
  flag: FeatureFlagName
  enabled: boolean
  course_override: boolean
}

interface CourseFeatureFlagsResponse {
  course_id: string
  flags: FeatureFlagState[]
}

function CourseFeatureFlagsCell({
  courseId,
  courseName,
}: {
  courseId: string
  courseName: string
}) {
  const { t } = useTranslation("admin")
  const formatError = useApiErrorMessage()
  const queryClient = useQueryClient()
  const [open, setOpen] = useState(false)

  const flagsQuery = useQuery({
    queryKey: ["admin", "courses", courseId, "feature-flags"],
    queryFn: () =>
      api.get<CourseFeatureFlagsResponse>(
        `/admin/courses/${courseId}/feature-flags`,
      ),
    // Only fetch when the dialog opens -- saves N admin-courses
    // queries on initial page load. The cell summary uses the
    // course's `feature_flags` field from the courses list query
    // for its at-a-glance count.
    enabled: open,
  })

  const setFlagMutation = useMutation({
    mutationFn: ({
      flag,
      enabled,
    }: {
      flag: FeatureFlagName
      enabled: boolean | null
    }) =>
      api.put<CourseFeatureFlagsResponse>(
        `/admin/courses/${courseId}/feature-flags`,
        { flags: { [flag]: enabled } },
      ),
    onSuccess: (data) => {
      // Update the per-course feature-flags cache in place so the
      // dialog's UI reflects the new state without a refetch flicker.
      queryClient.setQueryData(
        ["admin", "courses", courseId, "feature-flags"],
        data,
      )
      // Invalidate the broader courses list so the cell summary
      // ("2/3 features") and any consumers (e.g. the teacher's
      // course-edit-page tab gate) pick up the change.
      queryClient.invalidateQueries({ queryKey: ["courses"] })
      queryClient.invalidateQueries({ queryKey: ["courses", courseId] })
    },
  })

  const flags = flagsQuery.data?.flags ?? []
  const enabledCount = flags.filter((f) => f.enabled).length

  return (
    <>
      <Button
        variant="outline"
        size="sm"
        onClick={() => setOpen(true)}
        title={t("courses.featuresButtonTitle")}
      >
        {flagsQuery.data
          ? t("courses.featuresButton", {
              enabled: enabledCount,
              total: flags.length,
            })
          : t("courses.featuresButtonShort")}
      </Button>
      <AlertDialog open={open} onOpenChange={setOpen}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>
              {t("courses.featuresDialogTitle")}
            </AlertDialogTitle>
            <AlertDialogDescription>
              {t("courses.featuresDialogDescription", { course: courseName })}
            </AlertDialogDescription>
          </AlertDialogHeader>
          <div className="space-y-3 py-2">
            {flagsQuery.isLoading ? (
              <Skeleton className="h-20 w-full" />
            ) : flagsQuery.error ? (
              <p className="text-sm text-destructive">
                {formatError(flagsQuery.error)}
              </p>
            ) : (
              KNOWN_FEATURE_FLAGS.map((flag) => {
                const state = flags.find((f) => f.flag === flag)
                const enabled = state?.enabled ?? false
                const overridden = state?.course_override ?? false
                return (
                  <label
                    key={flag}
                    className="flex items-start gap-3 rounded border p-3 cursor-pointer hover:bg-muted/40"
                  >
                    <Checkbox
                      checked={enabled}
                      onCheckedChange={(value) =>
                        setFlagMutation.mutate({
                          flag,
                          enabled: value === true,
                        })
                      }
                      disabled={setFlagMutation.isPending}
                    />
                    <div className="space-y-1 flex-1">
                      <div className="flex items-center gap-2">
                        <span className="font-medium">
                          {t(`courses.featureFlagLabel.${flag}`)}
                        </span>
                        {overridden ? (
                          <Badge variant="secondary" className="text-xs">
                            {t("courses.featureOverridden")}
                          </Badge>
                        ) : (
                          <Badge variant="outline" className="text-xs">
                            {t("courses.featureInherited")}
                          </Badge>
                        )}
                      </div>
                      <p className="text-xs text-muted-foreground">
                        {t(`courses.featureFlagDescription.${flag}`)}
                      </p>
                      {overridden && (
                        <button
                          type="button"
                          className="text-xs text-muted-foreground underline-offset-4 hover:underline"
                          onClick={(e) => {
                            e.preventDefault()
                            setFlagMutation.mutate({ flag, enabled: null })
                          }}
                          disabled={setFlagMutation.isPending}
                        >
                          {t("courses.featureRevertToDefault")}
                        </button>
                      )}
                    </div>
                  </label>
                )
              })
            )}
            {setFlagMutation.isError && (
              <p className="text-sm text-destructive">
                {formatError(setFlagMutation.error)}
              </p>
            )}
          </div>
          <AlertDialogFooter>
            <AlertDialogCancel>{t("courses.featuresDialogClose")}</AlertDialogCancel>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </>
  )
}
