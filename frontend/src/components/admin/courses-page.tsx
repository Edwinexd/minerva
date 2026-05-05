import { Link } from "@tanstack/react-router"
import { RelativeTime } from "@/components/relative-time"
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query"
import { useTranslation } from "react-i18next"
import {
  adminEmbeddingModelsQuery,
  adminUsersQuery,
  coursesQuery,
} from "@/lib/queries"
import type { Course } from "@/lib/types"
import { modelDisplayName } from "@/lib/embedding-models"
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
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog"
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select"
import { Skeleton } from "@/components/ui/skeleton"
import { useState } from "react"
import { useApiErrorMessage } from "@/lib/use-api-error"

/// Single source of truth for known flags on the frontend, mirrored
/// from the backend's `feature_flags::ALL_FLAGS`. Adding a flag here
/// + i18n key + bumping the backend list is everything needed for
/// the admin UI to surface a new toggle.
const KNOWN_FEATURE_FLAGS = [
  "course_kg",
  "extraction_guard",
  "aegis",
  "concept_graph",
  "study_mode",
] as const
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
                <th className="py-2 pr-4 font-medium">{t("courses.columns.embedding")}</th>
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
                      <CourseEmbeddingCell course={course} />
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
    // Only fetch when the dialog opens; saves N admin-courses
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

// ── Embedding cell + force-migrate dialog ──────────────────────────
//
// The cell shows the course's currently-configured embedding provider
// + model and a "Migrate" button. The button opens an AlertDialog with
// a provider radio and a model select; on confirm we PUT to
// `/courses/{id}` with the new (provider, model). Admins bypass the
// `local_embedding_model_disabled` check on that route, so we can
// target *any* catalog model; including ones currently disabled in
// the picker (a typical workflow is "disable model X site-wide, then
// walk every course off it").
//
// Re-embedding cost: the rotation path (`rotate_embedding` in
// `minerva-db`) bumps `embedding_version` and re-queues every document
// in the course. The dialog body warns about that explicitly so a
// distracted admin doesn't fire it on a 1000-doc course by accident.

const PROVIDERS = ["local", "openai"] as const

function CourseEmbeddingCell({ course }: { course: Course }) {
  const { t } = useTranslation("admin")
  const [open, setOpen] = useState(false)
  // Friendly name for the model id; falls back to the raw HF id for
  // anything no longer in the catalog (admin disabled + dropped).
  // The full id stays accessible via the cell's title attribute, so
  // we don't lose fidelity by hiding it from the visual layer.
  const friendly = modelDisplayName(course.embedding_model)
  // `local` is the default provider for almost every course, so
  // showing it on every row is just noise. Only surface a provider
  // hint when it's something else (today: openai).
  const showProvider = course.embedding_provider !== "local"
  return (
    <>
      <div className="space-y-0.5" title={course.embedding_model}>
        <div className="text-sm">{friendly}</div>
        <div className="flex items-center gap-2 text-xs text-muted-foreground">
          {showProvider && <span>{course.embedding_provider}</span>}
          <button
            type="button"
            onClick={() => setOpen(true)}
            title={t("courses.migrateButtonTitle")}
            className="text-primary underline-offset-4 hover:underline"
          >
            {t("courses.migrateButton")}
          </button>
        </div>
      </div>
      {open && (
        <CourseMigrateDialog
          course={course}
          open={open}
          onOpenChange={setOpen}
        />
      )}
    </>
  )
}

function CourseMigrateDialog({
  course,
  open,
  onOpenChange,
}: {
  course: Course
  open: boolean
  onOpenChange: (o: boolean) => void
}) {
  const { t } = useTranslation("admin")
  const formatError = useApiErrorMessage()
  const queryClient = useQueryClient()
  const { data: catalog } = useQuery(adminEmbeddingModelsQuery)

  const [provider, setProvider] = useState(course.embedding_provider)
  const [model, setModel] = useState(course.embedding_model)

  const mutation = useMutation({
    mutationFn: () =>
      api.put(`/courses/${course.id}`, {
        embedding_provider: provider,
        // OpenAI canonicalises the model server-side; sending the
        // current value here is a no-op and keeps the payload simple.
        embedding_model:
          provider === "openai" ? course.embedding_model : model,
      }),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["courses"] })
      onOpenChange(false)
    },
  })

  // Local-provider option list = full catalog (admin can target
  // disabled models too; that's the whole point of force-migrate).
  // Sort current selection first so it stays visible after a click.
  const localOptions = (catalog?.models ?? []).slice().sort((a, b) => {
    if (a.model === course.embedding_model) return -1
    if (b.model === course.embedding_model) return 1
    return a.model.localeCompare(b.model)
  })

  const willRotate =
    provider !== course.embedding_provider ||
    (provider === "local" && model !== course.embedding_model)

  return (
    <AlertDialog open={open} onOpenChange={onOpenChange}>
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle>
            {t("courses.migrateDialogTitle", { course: course.name })}
          </AlertDialogTitle>
          <AlertDialogDescription>
            {t("courses.migrateDialogDescription")}
          </AlertDialogDescription>
        </AlertDialogHeader>
        <div className="space-y-4 py-2">
          <div className="space-y-1">
            <label className="text-sm font-medium">
              {t("courses.migrateProviderLabel")}
            </label>
            <Select value={provider} onValueChange={(v) => v && setProvider(v)}>
              <SelectTrigger className="w-full">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                {PROVIDERS.map((p) => (
                  <SelectItem key={p} value={p}>
                    {p}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </div>
          {provider === "local" && (
            <div className="space-y-1">
              <label className="text-sm font-medium">
                {t("courses.migrateModelLabel")}
              </label>
              <Select value={model} onValueChange={(v) => v && setModel(v)}>
                <SelectTrigger className="w-full">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  {localOptions.map((m) => (
                    <SelectItem key={m.model} value={m.model}>
                      <span title={m.model}>{modelDisplayName(m.model)}</span>
                      {!m.enabled && (
                        <span className="ml-2 text-[10px] text-muted-foreground">
                          {t("courses.migrateModelDisabledTag")}
                        </span>
                      )}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>
          )}
          {willRotate && (
            <div className="rounded-md border border-amber-300 bg-amber-50 px-3 py-2 text-xs text-amber-900 dark:border-amber-800 dark:bg-amber-950/40 dark:text-amber-200">
              {t("courses.migrateWarning")}
            </div>
          )}
          {mutation.isError && (
            <p className="text-sm text-destructive">
              {formatError(mutation.error)}
            </p>
          )}
        </div>
        <AlertDialogFooter>
          <AlertDialogCancel>
            {t("courses.migrateCancel")}
          </AlertDialogCancel>
          <AlertDialogAction
            disabled={!willRotate || mutation.isPending}
            onClick={(e) => {
              e.preventDefault()
              mutation.mutate()
            }}
          >
            {mutation.isPending
              ? t("courses.migrateSubmitting")
              : t("courses.migrateConfirm")}
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  )
}
