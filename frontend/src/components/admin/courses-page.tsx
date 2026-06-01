import { Link } from "@tanstack/react-router"
import { RelativeTime } from "@/components/relative-time"
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query"
import { useTranslation } from "react-i18next"
import {
  adminCoursesQuery,
  adminEmbeddingModelsQuery,
  adminMergeSuggestionsQuery,
  adminRerankerModelsQuery,
  adminUsersQuery,
  modelsQuery,
} from "@/lib/queries"
import type { Course, MergeSuggestionGroup } from "@/lib/types"
import { modelDisplayName } from "@/lib/embedding-models"
import { RERANKER_MODEL_DISPLAY } from "@/lib/reranker-models"
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
import { Input } from "@/components/ui/input"
import { Slider } from "@/components/ui/slider"
import { Textarea } from "@/components/ui/textarea"
import { Separator } from "@/components/ui/separator"
import { Skeleton } from "@/components/ui/skeleton"
import { useState } from "react"
import type { ReactNode } from "react"
import { useApiErrorMessage, useLocalizedMessage } from "@/lib/use-api-error"

/// Single source of truth for known flags on the frontend, mirrored
/// from the backend's `feature_flags::ALL_FLAGS`. Adding a flag here
/// + i18n key + bumping the backend list is everything needed for
/// the admin UI to surface a new toggle.
const KNOWN_FEATURE_FLAGS = [
  "course_kg",
  "extraction_guard",
  "aegis",
  "concept_graph",
] as const
type FeatureFlagName = (typeof KNOWN_FEATURE_FLAGS)[number]

export function CourseManagementPanel() {
  const { t } = useTranslation("admin")
  const { data: courses, isLoading: coursesLoading } = useQuery(adminCoursesQuery)
  const { data: users } = useQuery(adminUsersQuery)
  const [filter, setFilter] = useState("")
  const [statusFilter, setStatusFilter] = useState<"all" | "active" | "archived">(
    "all",
  )
  // Multi-select for bulk actions. Holds course ids regardless of the
  // current filter so a selection survives the admin narrowing /
  // widening the visible set; the header checkbox only ever toggles the
  // currently-filtered rows.
  const [selectedIds, setSelectedIds] = useState<Set<string>>(new Set())

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

  const filtered = courses.filter((c) => {
    if (statusFilter === "active" && !c.active) return false
    if (statusFilter === "archived" && c.active) return false
    if (!filter) return true
    const owner = userMap.get(c.owner_id)
    const ownerLabel = owner?.display_name ?? owner?.eppn ?? c.owner_id
    return (
      c.name.toLowerCase().includes(filter.toLowerCase()) ||
      ownerLabel.toLowerCase().includes(filter.toLowerCase())
    )
  })

  const filteredIds = filtered.map((c) => c.id)
  const selectedFilteredCount = filteredIds.filter((id) =>
    selectedIds.has(id),
  ).length
  const allFilteredSelected =
    filteredIds.length > 0 && selectedFilteredCount === filteredIds.length
  const headerIndeterminate = selectedFilteredCount > 0 && !allFilteredSelected

  const toggleAllFiltered = (checked: boolean) => {
    setSelectedIds((prev) => {
      const next = new Set(prev)
      if (checked) {
        filteredIds.forEach((id) => next.add(id))
      } else {
        filteredIds.forEach((id) => next.delete(id))
      }
      return next
    })
  }
  const toggleOne = (id: string, checked: boolean) => {
    setSelectedIds((prev) => {
      const next = new Set(prev)
      if (checked) next.add(id)
      else next.delete(id)
      return next
    })
  }

  // Selected rows resolved against the full (unfiltered) course list so
  // the bulk bar can act on selections the current filter would hide.
  const selectedCourses = courses.filter((c) => selectedIds.has(c.id))

  return (
    <div className="space-y-4">
      <SuggestedMergesCard />
      {selectedCourses.length > 0 && (
        <BulkActionBar
          selected={selectedCourses}
          courses={courses}
          onClearSelection={() => setSelectedIds(new Set())}
        />
      )}
      <Card>
      <CardHeader>
        <CardTitle>{t("courses.title", { total: courses.length })}</CardTitle>
        <CardDescription>{t("courses.description")}</CardDescription>
        <div className="mt-2 flex flex-wrap items-center gap-2">
          <input
            className="w-full max-w-sm rounded border bg-background px-3 py-1.5 text-sm"
            placeholder={t("courses.filterPlaceholder")}
            aria-label={t("courses.filterPlaceholder")}
            value={filter}
            onChange={(e) => setFilter(e.target.value)}
          />
          <Select
            value={statusFilter}
            onValueChange={(v) =>
              v && setStatusFilter(v as "all" | "active" | "archived")
            }
          >
            <SelectTrigger
              className="w-40"
              aria-label={t("courses.statusFilterLabel")}
            >
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="all">{t("courses.statusFilter.all")}</SelectItem>
              <SelectItem value="active">
                {t("courses.statusFilter.active")}
              </SelectItem>
              <SelectItem value="archived">
                {t("courses.statusFilter.archived")}
              </SelectItem>
            </SelectContent>
          </Select>
        </div>
      </CardHeader>
      <CardContent>
        <div className="overflow-x-auto">
          <table className="w-full text-sm">
            <thead>
              <tr className="border-b text-left">
                <th className="py-2 pr-3 font-medium">
                  <Checkbox
                    checked={allFilteredSelected}
                    indeterminate={headerIndeterminate}
                    onCheckedChange={(v) => toggleAllFiltered(v === true)}
                    aria-label={t("courses.bulk.selectAll")}
                    disabled={filteredIds.length === 0}
                  />
                </th>
                <th className="py-2 pr-4 font-medium">{t("courses.columns.course")}</th>
                <th className="py-2 pr-4 font-medium">{t("courses.columns.owner")}</th>
                <th className="py-2 pr-4 font-medium">{t("courses.columns.status")}</th>
                <th className="py-2 pr-4 font-medium">{t("courses.columns.tokenLimit")}</th>
                <th className="py-2 pr-4 font-medium">{t("courses.columns.created")}</th>
                <th className="py-2 pr-4 font-medium">{t("courses.columns.embedding")}</th>
                <th className="py-2 pr-4 font-medium">{t("courses.columns.features")}</th>
                <th className="py-2 pr-4 font-medium">{t("courses.columns.settings")}</th>
                <th className="py-2 font-medium">{t("courses.columns.actions")}</th>
              </tr>
            </thead>
            <tbody>
              {filtered.map((course) => {
                const owner = userMap.get(course.owner_id)
                const ownerLabel =
                  owner?.display_name ?? owner?.eppn ?? course.owner_id.slice(0, 8)
                return (
                  <tr
                    key={course.id}
                    className={
                      selectedIds.has(course.id)
                        ? "border-b bg-muted/40"
                        : "border-b"
                    }
                  >
                    <td className="py-2 pr-3">
                      <Checkbox
                        checked={selectedIds.has(course.id)}
                        onCheckedChange={(v) => toggleOne(course.id, v === true)}
                        aria-label={t("courses.bulk.selectRow", {
                          course: course.name,
                        })}
                      />
                    </td>
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
                    <td className="py-2 pr-4">
                      <Link
                        to="/teacher/courses/$courseId/config"
                        params={{ courseId: course.id }}
                        className="text-primary underline-offset-4 hover:underline"
                      >
                        {t("courses.settingsLink")}
                      </Link>
                    </td>
                    <td className="py-2">
                      <CourseActionsCell course={course} allCourses={courses} />
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
    </div>
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
      queryClient.invalidateQueries({ queryKey: ["admin", "courses"] })
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
              <SelectTrigger className="w-full" aria-label={t("courses.migrateProviderLabel")}>
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
                <SelectTrigger className="w-full" aria-label={t("courses.migrateModelLabel")}>
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

// ── Per-course admin actions: archive / restore + merge ────────────
//
// `CourseActionsCell` renders the inline buttons in the last table
// column. Archive / restore toggle the soft-delete flag; "Merge" opens
// a dialog where the admin picks a SURVIVOR course to fold this row
// (the source) into. Only active courses can be a merge source or a
// survivor candidate.

function CourseActionsCell({
  course,
  allCourses,
}: {
  course: Course
  allCourses: Course[]
}) {
  const { t } = useTranslation("admin")
  const formatError = useApiErrorMessage()
  const queryClient = useQueryClient()
  const [mergeOpen, setMergeOpen] = useState(false)

  const toggleArchiveMutation = useMutation({
    mutationFn: () =>
      course.active
        ? api.post(`/admin/courses/${course.id}/archive`, {})
        : api.post(`/admin/courses/${course.id}/unarchive`, {}),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["admin", "courses"] })
      queryClient.invalidateQueries({ queryKey: ["courses"] })
    },
  })

  return (
    <div className="flex items-center gap-2">
      {course.active && (
        <Button
          variant="outline"
          size="sm"
          onClick={() => setMergeOpen(true)}
          title={t("courses.mergeButtonTitle")}
        >
          {t("courses.mergeButton")}
        </Button>
      )}
      <Button
        variant="outline"
        size="sm"
        onClick={() => toggleArchiveMutation.mutate()}
        disabled={toggleArchiveMutation.isPending}
      >
        {course.active
          ? t("courses.archiveButton")
          : t("courses.restoreButton")}
      </Button>
      {toggleArchiveMutation.isError && (
        <span className="text-xs text-destructive">
          {formatError(toggleArchiveMutation.error)}
        </span>
      )}
      {mergeOpen && (
        <MergeCourseDialog
          source={course}
          candidates={allCourses.filter((c) => c.active && c.id !== course.id)}
          open={mergeOpen}
          onOpenChange={setMergeOpen}
        />
      )}
    </div>
  )
}

interface MergeResult {
  merged: boolean
  documents_moved: number
  documents_orphaned: number
  documents_requeued: number
  conversations_moved: number
  members_merged: number
  offerings_moved: number
}

function MergeCourseDialog({
  source,
  candidates,
  open,
  onOpenChange,
}: {
  source: Course
  candidates: Course[]
  open: boolean
  onOpenChange: (o: boolean) => void
}) {
  const { t } = useTranslation("admin")
  const formatError = useApiErrorMessage()
  const queryClient = useQueryClient()
  const [survivorId, setSurvivorId] = useState("")

  const mutation = useMutation({
    mutationFn: () =>
      api.post<MergeResult>("/admin/courses/merge", {
        survivor_id: survivorId,
        source_id: source.id,
      }),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["admin", "courses"] })
      queryClient.invalidateQueries({ queryKey: ["courses"] })
      onOpenChange(false)
    },
  })

  const survivor = candidates.find((c) => c.id === survivorId)

  return (
    <AlertDialog open={open} onOpenChange={onOpenChange}>
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle>
            {t("courses.mergeDialogTitle", { course: source.name })}
          </AlertDialogTitle>
          <AlertDialogDescription>
            {t("courses.mergeDialogDescription")}
          </AlertDialogDescription>
        </AlertDialogHeader>
        <div className="space-y-4 py-2">
          {candidates.length === 0 ? (
            <p className="text-sm text-muted-foreground">
              {t("courses.mergeNoCandidates")}
            </p>
          ) : (
            <div className="space-y-1">
              <label className="text-sm font-medium">
                {t("courses.mergeSurvivorLabel")}
              </label>
              <Select
                value={survivorId}
                onValueChange={(v) => v && setSurvivorId(v)}
              >
                <SelectTrigger
                  className="w-full"
                  aria-label={t("courses.mergeSurvivorLabel")}
                >
                  <SelectValue
                    placeholder={t("courses.mergeSurvivorPlaceholder")}
                  />
                </SelectTrigger>
                <SelectContent>
                  {candidates.map((c) => (
                    <SelectItem key={c.id} value={c.id}>
                      {c.semester_label
                        ? `${c.name} (${c.semester_label})`
                        : c.name}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>
          )}
          {survivor && (
            <div className="rounded-md border border-amber-300 bg-amber-50 px-3 py-2 text-xs text-amber-900 dark:border-amber-800 dark:bg-amber-950/40 dark:text-amber-200">
              {t("courses.mergeWarning", {
                source: source.name,
                survivor: survivor.name,
              })}
            </div>
          )}
          {mutation.isError && (
            <p className="text-sm text-destructive">
              {formatError(mutation.error)}
            </p>
          )}
        </div>
        <AlertDialogFooter>
          <AlertDialogCancel>{t("courses.mergeCancel")}</AlertDialogCancel>
          <AlertDialogAction
            disabled={!survivorId || mutation.isPending}
            onClick={(e) => {
              e.preventDefault()
              mutation.mutate()
            }}
          >
            {mutation.isPending
              ? t("courses.mergeSubmitting")
              : t("courses.mergeConfirm")}
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  )
}

// ── Suggested merges ───────────────────────────────────────────────
//
// A heuristic panel that surfaces groups of active courses that look
// like the same course delivered under several codes (e.g. SUPCOM /
// SUPCOM-HI / SUPCOM-DIST share a name and a base code). The admin
// picks which course in the group survives; the rest are merged into it
// (one merge call each) and archived. Only renders when the backend
// returns at least one group.

function SuggestedMergesCard() {
  const { t } = useTranslation("admin")
  const { data: groups } = useQuery(adminMergeSuggestionsQuery)
  if (!groups || groups.length === 0) return null
  return (
    <Card>
      <CardHeader>
        <CardTitle>
          {t("courses.suggestions.title", { count: groups.length })}
        </CardTitle>
        <CardDescription>{t("courses.suggestions.description")}</CardDescription>
      </CardHeader>
      <CardContent className="space-y-3">
        {groups.map((group, i) => (
          <SuggestedMergeGroup key={i} group={group} />
        ))}
      </CardContent>
    </Card>
  )
}

function SuggestedMergeGroup({ group }: { group: MergeSuggestionGroup }) {
  const { t } = useTranslation("admin")
  const formatError = useApiErrorMessage()
  const queryClient = useQueryClient()
  // Default the survivor to the Daisy-managed course if one is present
  // (that's the canonical record for the pre-Daisy -> Daisy case);
  // otherwise the first listed course.
  const defaultSurvivor =
    group.courses.find((c) => c.auto_managed)?.id ?? group.courses[0].id
  const [survivorId, setSurvivorId] = useState(defaultSurvivor)
  const [confirmOpen, setConfirmOpen] = useState(false)

  const mergeMutation = useMutation({
    mutationFn: async () => {
      // Merge every other group member into the chosen survivor, one
      // call each (each merge is its own transaction server-side).
      for (const c of group.courses) {
        if (c.id === survivorId) continue
        await api.post("/admin/courses/merge", {
          survivor_id: survivorId,
          source_id: c.id,
        })
      }
    },
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["admin", "courses"] })
      queryClient.invalidateQueries({ queryKey: ["courses"] })
      setConfirmOpen(false)
    },
  })

  const survivor = group.courses.find((c) => c.id === survivorId)
  const otherCount = group.courses.length - 1

  return (
    <div className="space-y-3 rounded border p-3">
      <div className="flex flex-wrap items-center gap-2">
        <Badge variant="secondary">{group.code}</Badge>
        {group.semester_label && (
          <Badge variant="outline">{group.semester_label}</Badge>
        )}
        <span className="text-sm text-muted-foreground">
          {t("courses.suggestions.groupCount", { count: group.courses.length })}
        </span>
      </div>
      <ul className="space-y-1 text-sm">
        {group.courses.map((c) => (
          <li key={c.id} className="flex flex-wrap items-center gap-2">
            {c.course_code && (
              <Badge variant="outline" className="text-xs">
                {c.course_code}
              </Badge>
            )}
            <span className="font-medium">{c.name}</span>
            {c.semester_label && (
              <span className="text-xs text-muted-foreground">
                {c.semester_label}
              </span>
            )}
            {c.id === survivorId && (
              <Badge className="text-xs">
                {t("courses.suggestions.survivorBadge")}
              </Badge>
            )}
          </li>
        ))}
      </ul>
      <div className="flex flex-wrap items-center gap-2">
        <label className="text-sm font-medium">
          {t("courses.suggestions.survivorLabel")}
        </label>
        <Select value={survivorId} onValueChange={(v) => v && setSurvivorId(v)}>
          <SelectTrigger
            className="w-72 max-w-full"
            aria-label={t("courses.suggestions.survivorLabel")}
          >
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            {group.courses.map((c) => (
              <SelectItem key={c.id} value={c.id}>
                {c.course_code ? `${c.course_code} ` : ""}
                {c.name}
                {c.semester_label ? ` (${c.semester_label})` : ""}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
        <Button
          size="sm"
          onClick={() => setConfirmOpen(true)}
          disabled={mergeMutation.isPending}
        >
          {t("courses.suggestions.mergeButton")}
        </Button>
      </div>
      {mergeMutation.isError && (
        <p className="text-sm text-destructive">
          {formatError(mergeMutation.error)}
        </p>
      )}
      <AlertDialog open={confirmOpen} onOpenChange={setConfirmOpen}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>
              {t("courses.suggestions.confirmTitle")}
            </AlertDialogTitle>
            <AlertDialogDescription>
              {t("courses.suggestions.confirmDescription", {
                count: otherCount,
                survivor: survivor?.name ?? "",
              })}
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>{t("courses.mergeCancel")}</AlertDialogCancel>
            <AlertDialogAction
              disabled={mergeMutation.isPending}
              onClick={(e) => {
                e.preventDefault()
                mergeMutation.mutate()
              }}
            >
              {mergeMutation.isPending
                ? t("courses.mergeSubmitting")
                : t("courses.suggestions.confirmButton")}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  )
}

// ── Bulk actions ───────────────────────────────────────────────────
//
// When ≥1 course is selected, `BulkActionBar` floats above the table
// with three operations: edit settings (the big patch dialog), bulk
// archive, and bulk restore. Each backend call returns a per-course
// `{ ok, error }` list so a partial batch (one course on a now-disabled
// model, an archived row that can't be edited, etc.) is shown precisely
// rather than collapsing to a single success/failure.

interface BulkResultItem {
  course_id: string
  ok: boolean
  error?: { code: string; params?: Record<string, string> }
}

interface BulkResponse {
  succeeded: number
  failed: number
  results: BulkResultItem[]
}

function BulkResultSummary({
  result,
  courses,
  onDismiss,
}: {
  result: BulkResponse
  courses: Course[]
  onDismiss: () => void
}) {
  const { t } = useTranslation("admin")
  const localized = useLocalizedMessage()
  const nameOf = (id: string) =>
    courses.find((c) => c.id === id)?.name ?? id.slice(0, 8)
  const failures = result.results.filter((r) => !r.ok)
  return (
    <div className="w-full rounded border bg-muted/30 p-2 text-xs">
      <div className="flex items-center justify-between gap-2">
        <span>
          {t("courses.bulk.resultSummary", {
            ok: result.succeeded,
            failed: result.failed,
          })}
        </span>
        <button
          type="button"
          className="underline-offset-2 hover:underline"
          onClick={onDismiss}
        >
          {t("courses.bulk.dismiss")}
        </button>
      </div>
      {failures.length > 0 && (
        <ul className="mt-1 space-y-0.5">
          {failures.map((f) => (
            <li key={f.course_id} className="text-destructive">
              <span className="font-medium">{nameOf(f.course_id)}</span>
              {": "}
              {f.error ? localized(f.error) : t("courses.bulk.unknownError")}
            </li>
          ))}
        </ul>
      )}
    </div>
  )
}

function BulkActionBar({
  selected,
  courses,
  onClearSelection,
}: {
  selected: Course[]
  courses: Course[]
  onClearSelection: () => void
}) {
  const { t } = useTranslation("admin")
  const formatError = useApiErrorMessage()
  const queryClient = useQueryClient()
  const [editOpen, setEditOpen] = useState(false)
  const [confirm, setConfirm] = useState<null | "archive" | "restore">(null)
  const [result, setResult] = useState<BulkResponse | null>(null)

  const activeSelected = selected.filter((c) => c.active)
  const archivedSelected = selected.filter((c) => !c.active)

  const lifecycleMutation = useMutation({
    mutationFn: ({
      kind,
      ids,
    }: {
      kind: "archive" | "restore"
      ids: string[]
    }) =>
      api.post<BulkResponse>(
        kind === "archive"
          ? "/admin/courses/bulk-archive"
          : "/admin/courses/bulk-unarchive",
        { course_ids: ids },
      ),
    onSuccess: (data) => {
      queryClient.invalidateQueries({ queryKey: ["admin", "courses"] })
      queryClient.invalidateQueries({ queryKey: ["courses"] })
      setConfirm(null)
      setResult(data)
      if (data.failed === 0) onClearSelection()
    },
  })

  return (
    <Card>
      <CardContent className="flex flex-wrap items-center gap-2 py-3">
        <span className="text-sm font-medium">
          {t("courses.bulk.selectedCount", { count: selected.length })}
        </span>
        <div className="flex flex-wrap items-center gap-2">
          <Button
            size="sm"
            onClick={() => {
              setResult(null)
              setEditOpen(true)
            }}
          >
            {t("courses.bulk.editSettings")}
          </Button>
          <Button
            size="sm"
            variant="outline"
            disabled={activeSelected.length === 0 || lifecycleMutation.isPending}
            onClick={() => {
              setResult(null)
              setConfirm("archive")
            }}
          >
            {t("courses.bulk.archive", { count: activeSelected.length })}
          </Button>
          <Button
            size="sm"
            variant="outline"
            disabled={
              archivedSelected.length === 0 || lifecycleMutation.isPending
            }
            onClick={() => {
              setResult(null)
              setConfirm("restore")
            }}
          >
            {t("courses.bulk.restore", { count: archivedSelected.length })}
          </Button>
          <Button size="sm" variant="ghost" onClick={onClearSelection}>
            {t("courses.bulk.clear")}
          </Button>
        </div>
        {lifecycleMutation.isError && (
          <span className="text-xs text-destructive">
            {formatError(lifecycleMutation.error)}
          </span>
        )}
        {result && (
          <BulkResultSummary
            result={result}
            courses={courses}
            onDismiss={() => setResult(null)}
          />
        )}
      </CardContent>

      <AlertDialog
        open={confirm !== null}
        onOpenChange={(o) => !o && setConfirm(null)}
      >
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>
              {confirm === "restore"
                ? t("courses.bulk.restoreConfirmTitle")
                : t("courses.bulk.archiveConfirmTitle")}
            </AlertDialogTitle>
            <AlertDialogDescription>
              {confirm === "restore"
                ? t("courses.bulk.restoreConfirmBody", {
                    count: archivedSelected.length,
                  })
                : t("courses.bulk.archiveConfirmBody", {
                    count: activeSelected.length,
                  })}
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>{t("courses.bulk.cancel")}</AlertDialogCancel>
            <AlertDialogAction
              disabled={lifecycleMutation.isPending}
              onClick={(e) => {
                e.preventDefault()
                lifecycleMutation.mutate(
                  confirm === "restore"
                    ? {
                        kind: "restore",
                        ids: archivedSelected.map((c) => c.id),
                      }
                    : { kind: "archive", ids: activeSelected.map((c) => c.id) },
                )
              }}
            >
              {lifecycleMutation.isPending
                ? t("courses.bulk.applying")
                : t("courses.bulk.confirm")}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>

      {editOpen && (
        <BulkEditDialog
          selected={selected}
          courses={courses}
          open={editOpen}
          onOpenChange={setEditOpen}
          onClearSelection={onClearSelection}
        />
      )}
    </Card>
  )
}

// One labelled row in the bulk-edit form: a checkbox that decides
// whether the field is part of the patch at all, plus the control
// (revealed only when the field is included). Fields the admin doesn't
// tick are omitted from the request, so a bulk edit only ever writes
// what was explicitly set.
function FieldRow({
  label,
  help,
  enabled,
  onToggle,
  children,
}: {
  label: string
  help?: string
  enabled: boolean
  onToggle: (v: boolean) => void
  children: ReactNode
}) {
  return (
    <div className="space-y-1.5 rounded border p-3">
      <label className="flex cursor-pointer items-center gap-2 text-sm font-medium">
        <Checkbox
          checked={enabled}
          onCheckedChange={(v) => onToggle(v === true)}
        />
        <span>{label}</span>
      </label>
      {help && <p className="text-xs text-muted-foreground">{help}</p>}
      {enabled && <div className="pt-1">{children}</div>}
    </div>
  )
}

type FlagChoice = "nochange" | "on" | "off" | "default"

type ScalarField =
  | "model"
  | "temperature"
  | "strategy"
  | "tool_use_enabled"
  | "context_ratio"
  | "max_chunks"
  | "min_score"
  | "daily_token_limit"
  | "system_prompt"
  | "semester_label"
  | "embedding"
  | "reranker_model"

function BulkEditDialog({
  selected,
  courses,
  open,
  onOpenChange,
  onClearSelection,
}: {
  selected: Course[]
  courses: Course[]
  open: boolean
  onOpenChange: (o: boolean) => void
  onClearSelection: () => void
}) {
  const { t } = useTranslation("admin")
  const formatError = useApiErrorMessage()
  const queryClient = useQueryClient()
  const { data: modelsData } = useQuery(modelsQuery)
  const { data: embeddingModelsData } = useQuery(adminEmbeddingModelsQuery)
  const { data: rerankerModelsData } = useQuery(adminRerankerModelsQuery)

  const [included, setIncluded] = useState<Set<ScalarField>>(new Set())
  const [model, setModel] = useState("")
  const [temperature, setTemperature] = useState(0.3)
  const [strategy, setStrategy] = useState("simple")
  const [toolUse, setToolUse] = useState(false)
  const [contextRatio, setContextRatio] = useState(0.7)
  const [maxChunks, setMaxChunks] = useState(10)
  const [minScore, setMinScore] = useState(0)
  const [dailyTokenLimit, setDailyTokenLimit] = useState(0)
  const [systemPrompt, setSystemPrompt] = useState("")
  const [semesterLabel, setSemesterLabel] = useState("")
  const [embeddingProvider, setEmbeddingProvider] = useState("local")
  const [embeddingModel, setEmbeddingModel] = useState("")
  const [rerankerModel, setRerankerModel] = useState("")
  const [flagChoices, setFlagChoices] = useState<
    Record<FeatureFlagName, FlagChoice>
  >(
    () =>
      Object.fromEntries(
        KNOWN_FEATURE_FLAGS.map((f) => [f, "nochange"]),
      ) as Record<FeatureFlagName, FlagChoice>,
  )
  const [result, setResult] = useState<BulkResponse | null>(null)

  const toggleField = (field: ScalarField, enable: boolean) => {
    setIncluded((prev) => {
      const next = new Set(prev)
      if (enable) next.add(field)
      else next.delete(field)
      return next
    })
    // Seed sane defaults from the async catalogs the first time a field
    // is switched on, so a freshly-revealed control isn't blank.
    if (enable) {
      if (field === "model" && !model && modelsData?.models[0]) {
        setModel(modelsData.models[0].id)
      }
      if (
        field === "reranker_model" &&
        !rerankerModel &&
        rerankerModelsData?.models[0]
      ) {
        setRerankerModel(rerankerModelsData.models[0].model)
      }
      if (
        field === "embedding" &&
        embeddingProvider === "local" &&
        !embeddingModel &&
        embeddingModelsData?.models[0]
      ) {
        setEmbeddingModel(embeddingModelsData.models[0].model)
      }
    }
  }

  const setFlag = (flag: FeatureFlagName, choice: FlagChoice) =>
    setFlagChoices((prev) => ({ ...prev, [flag]: choice }))

  const buildPatch = () => {
    const patch: Record<string, unknown> = {}
    if (included.has("model")) patch.model = model
    if (included.has("temperature")) patch.temperature = temperature
    if (included.has("strategy")) patch.strategy = strategy
    if (included.has("tool_use_enabled")) patch.tool_use_enabled = toolUse
    if (included.has("context_ratio")) patch.context_ratio = contextRatio
    if (included.has("max_chunks")) patch.max_chunks = maxChunks
    if (included.has("min_score")) patch.min_score = minScore
    if (included.has("daily_token_limit"))
      patch.daily_token_limit = dailyTokenLimit
    if (included.has("system_prompt")) patch.system_prompt = systemPrompt
    if (included.has("semester_label"))
      patch.semester_label = semesterLabel.trim().toUpperCase()
    if (included.has("embedding")) {
      patch.embedding_provider = embeddingProvider
      // For openai the backend canonicalises the model, so the provider
      // alone is enough; for local we must carry the chosen model.
      if (embeddingProvider === "local") patch.embedding_model = embeddingModel
    }
    if (included.has("reranker_model")) patch.reranker_model = rerankerModel
    return patch
  }

  const buildFlags = () => {
    const flags: Record<string, boolean | null> = {}
    for (const f of KNOWN_FEATURE_FLAGS) {
      const c = flagChoices[f]
      if (c === "on") flags[f] = true
      else if (c === "off") flags[f] = false
      else if (c === "default") flags[f] = null
    }
    return flags
  }

  const patch = buildPatch()
  const flags = buildFlags()
  const changeCount = Object.keys(patch).length + Object.keys(flags).length
  const archivedCount = selected.filter((c) => !c.active).length
  // A local embedding change with no model chosen would fail for every
  // course; block apply with a hint rather than firing a doomed batch.
  const embeddingMissingModel =
    included.has("embedding") &&
    embeddingProvider === "local" &&
    !embeddingModel
  const semesterInvalid =
    included.has("semester_label") &&
    !/^(?:VT|HT)\d{4}$/i.test(semesterLabel.trim())

  const mutation = useMutation({
    mutationFn: () =>
      api.post<BulkResponse>("/admin/courses/bulk", {
        course_ids: selected.map((c) => c.id),
        patch,
        feature_flags: flags,
      }),
    onSuccess: (data) => {
      queryClient.invalidateQueries({ queryKey: ["admin", "courses"] })
      queryClient.invalidateQueries({ queryKey: ["courses"] })
      setResult(data)
      if (data.failed === 0) {
        onClearSelection()
        onOpenChange(false)
      }
    },
  })

  const embeddingOptions = embeddingModelsData?.models ?? []
  const rerankerOptions = rerankerModelsData?.models ?? []

  return (
    <AlertDialog open={open} onOpenChange={onOpenChange}>
      <AlertDialogContent className="max-h-[85vh] overflow-y-auto sm:max-w-2xl">
        <AlertDialogHeader>
          <AlertDialogTitle>
            {t("courses.bulk.editTitle", { count: selected.length })}
          </AlertDialogTitle>
          <AlertDialogDescription>
            {t("courses.bulk.editDescription")}
          </AlertDialogDescription>
        </AlertDialogHeader>

        <div className="space-y-3 py-2">
          {archivedCount > 0 && (
            <div className="rounded-md border border-amber-300 bg-amber-50 px-3 py-2 text-xs text-amber-900 dark:border-amber-800 dark:bg-amber-950/40 dark:text-amber-200">
              {t("courses.bulk.archivedWarning", { count: archivedCount })}
            </div>
          )}

          <FieldRow
            label={t("courses.bulk.fields.model")}
            enabled={included.has("model")}
            onToggle={(v) => toggleField("model", v)}
          >
            <Select value={model} onValueChange={(v) => v && setModel(v)}>
              <SelectTrigger
                className="w-full"
                aria-label={t("courses.bulk.fields.model")}
              >
                <SelectValue placeholder={t("courses.bulk.selectPlaceholder")} />
              </SelectTrigger>
              <SelectContent>
                {modelsData?.models.map((m) => (
                  <SelectItem key={m.id} value={m.id}>
                    {m.name}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </FieldRow>

          <FieldRow
            label={t("courses.bulk.fields.temperature", {
              value: temperature.toFixed(2),
            })}
            enabled={included.has("temperature")}
            onToggle={(v) => toggleField("temperature", v)}
          >
            <Slider
              value={[temperature]}
              onValueChange={(v) =>
                setTemperature(Array.isArray(v) ? v[0] : v)
              }
              min={0}
              max={1}
              step={0.05}
            />
          </FieldRow>

          <FieldRow
            label={t("courses.bulk.fields.strategy")}
            enabled={included.has("strategy")}
            onToggle={(v) => toggleField("strategy", v)}
          >
            <Select value={strategy} onValueChange={(v) => v && setStrategy(v)}>
              <SelectTrigger
                className="w-full"
                aria-label={t("courses.bulk.fields.strategy")}
              >
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="simple">
                  {t("courses.bulk.strategySimple")}
                </SelectItem>
                <SelectItem value="flare">
                  {t("courses.bulk.strategyFlare")}
                </SelectItem>
              </SelectContent>
            </Select>
          </FieldRow>

          <FieldRow
            label={t("courses.bulk.fields.toolUse")}
            help={t("courses.bulk.fields.toolUseHelp")}
            enabled={included.has("tool_use_enabled")}
            onToggle={(v) => toggleField("tool_use_enabled", v)}
          >
            <label className="flex cursor-pointer items-center gap-2 text-sm">
              <Checkbox
                checked={toolUse}
                onCheckedChange={(v) => setToolUse(v === true)}
              />
              <span>
                {toolUse
                  ? t("courses.bulk.toolUseOn")
                  : t("courses.bulk.toolUseOff")}
              </span>
            </label>
          </FieldRow>

          <FieldRow
            label={t("courses.bulk.fields.contextRatio", {
              percent: Math.round(contextRatio * 100),
            })}
            enabled={included.has("context_ratio")}
            onToggle={(v) => toggleField("context_ratio", v)}
          >
            <Slider
              value={[contextRatio]}
              onValueChange={(v) =>
                setContextRatio(Array.isArray(v) ? v[0] : v)
              }
              min={0.1}
              max={0.95}
              step={0.05}
            />
          </FieldRow>

          <FieldRow
            label={t("courses.bulk.fields.maxChunks")}
            enabled={included.has("max_chunks")}
            onToggle={(v) => toggleField("max_chunks", v)}
          >
            <Input
              type="number"
              value={maxChunks}
              onChange={(e) => setMaxChunks(parseInt(e.target.value) || 10)}
              min={1}
              max={50}
            />
          </FieldRow>

          <FieldRow
            label={t("courses.bulk.fields.minScore", {
              value: minScore.toFixed(2),
            })}
            enabled={included.has("min_score")}
            onToggle={(v) => toggleField("min_score", v)}
          >
            <Slider
              value={[minScore]}
              onValueChange={(v) => setMinScore(Array.isArray(v) ? v[0] : v)}
              min={0}
              max={1}
              step={0.01}
            />
          </FieldRow>

          <FieldRow
            label={t("courses.bulk.fields.dailyTokenLimit")}
            help={t("courses.bulk.fields.dailyTokenLimitHelp")}
            enabled={included.has("daily_token_limit")}
            onToggle={(v) => toggleField("daily_token_limit", v)}
          >
            <Input
              type="number"
              value={dailyTokenLimit}
              onChange={(e) =>
                setDailyTokenLimit(parseInt(e.target.value) || 0)
              }
              min={0}
            />
          </FieldRow>

          <FieldRow
            label={t("courses.bulk.fields.semesterLabel")}
            help={t("courses.bulk.fields.semesterLabelHelp")}
            enabled={included.has("semester_label")}
            onToggle={(v) => toggleField("semester_label", v)}
          >
            <Input
              value={semesterLabel}
              onChange={(e) => setSemesterLabel(e.target.value)}
              placeholder="VT2026"
              pattern="(?:VT|HT|vt|ht)\d{4}"
            />
            {semesterInvalid && (
              <p className="mt-1 text-xs text-destructive">
                {t("courses.bulk.semesterInvalid")}
              </p>
            )}
          </FieldRow>

          <FieldRow
            label={t("courses.bulk.fields.reranker")}
            enabled={included.has("reranker_model")}
            onToggle={(v) => toggleField("reranker_model", v)}
          >
            <Select
              value={rerankerModel}
              onValueChange={(v) => v && setRerankerModel(v)}
            >
              <SelectTrigger
                className="w-full"
                aria-label={t("courses.bulk.fields.reranker")}
              >
                <SelectValue placeholder={t("courses.bulk.selectPlaceholder")} />
              </SelectTrigger>
              <SelectContent>
                {rerankerOptions.map((m) => (
                  <SelectItem key={m.model} value={m.model}>
                    {RERANKER_MODEL_DISPLAY[m.model]?.name ?? m.model}
                    {!m.enabled && (
                      <span className="ml-2 text-[10px] text-muted-foreground">
                        {t("courses.migrateModelDisabledTag")}
                      </span>
                    )}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </FieldRow>

          <FieldRow
            label={t("courses.bulk.fields.embedding")}
            help={t("courses.bulk.fields.embeddingHelp")}
            enabled={included.has("embedding")}
            onToggle={(v) => toggleField("embedding", v)}
          >
            <div className="space-y-2">
              <Select
                value={embeddingProvider}
                onValueChange={(v) => {
                  if (!v) return
                  setEmbeddingProvider(v)
                  if (
                    v === "local" &&
                    !embeddingModel &&
                    embeddingOptions[0]
                  ) {
                    setEmbeddingModel(embeddingOptions[0].model)
                  }
                }}
              >
                <SelectTrigger
                  className="w-full"
                  aria-label={t("courses.migrateProviderLabel")}
                >
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
              {embeddingProvider === "local" && (
                <Select
                  value={embeddingModel}
                  onValueChange={(v) => v && setEmbeddingModel(v)}
                >
                  <SelectTrigger
                    className="w-full"
                    aria-label={t("courses.migrateModelLabel")}
                  >
                    <SelectValue
                      placeholder={t("courses.bulk.selectPlaceholder")}
                    />
                  </SelectTrigger>
                  <SelectContent>
                    {embeddingOptions.map((m) => (
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
              )}
              <div className="rounded-md border border-amber-300 bg-amber-50 px-3 py-2 text-xs text-amber-900 dark:border-amber-800 dark:bg-amber-950/40 dark:text-amber-200">
                {t("courses.bulk.embeddingWarning")}
              </div>
            </div>
          </FieldRow>

          <FieldRow
            label={t("courses.bulk.fields.systemPrompt")}
            help={t("courses.bulk.fields.systemPromptHelp")}
            enabled={included.has("system_prompt")}
            onToggle={(v) => toggleField("system_prompt", v)}
          >
            <Textarea
              value={systemPrompt}
              onChange={(e) => setSystemPrompt(e.target.value)}
              rows={4}
              placeholder={t("courses.bulk.systemPromptPlaceholder")}
            />
          </FieldRow>

          <Separator />

          <div className="space-y-2">
            <p className="text-sm font-medium">
              {t("courses.bulk.featureFlagsTitle")}
            </p>
            <p className="text-xs text-muted-foreground">
              {t("courses.bulk.featureFlagsHelp")}
            </p>
            {KNOWN_FEATURE_FLAGS.map((flag) => (
              <div
                key={flag}
                className="flex items-center justify-between gap-3"
              >
                <span className="min-w-0 truncate text-sm">
                  {t(`courses.featureFlagLabel.${flag}`)}
                </span>
                <Select
                  value={flagChoices[flag]}
                  onValueChange={(v) => v && setFlag(flag, v as FlagChoice)}
                >
                  <SelectTrigger
                    className="w-44 shrink-0"
                    aria-label={t(`courses.featureFlagLabel.${flag}`)}
                  >
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem value="nochange">
                      {t("courses.bulk.flagNoChange")}
                    </SelectItem>
                    <SelectItem value="on">
                      {t("courses.bulk.flagOn")}
                    </SelectItem>
                    <SelectItem value="off">
                      {t("courses.bulk.flagOff")}
                    </SelectItem>
                    <SelectItem value="default">
                      {t("courses.bulk.flagDefault")}
                    </SelectItem>
                  </SelectContent>
                </Select>
              </div>
            ))}
          </div>

          {mutation.isError && (
            <p className="text-sm text-destructive">
              {formatError(mutation.error)}
            </p>
          )}
          {result && (
            <BulkResultSummary
              result={result}
              courses={courses}
              onDismiss={() => setResult(null)}
            />
          )}
        </div>

        <AlertDialogFooter>
          <AlertDialogCancel>{t("courses.bulk.cancel")}</AlertDialogCancel>
          <AlertDialogAction
            disabled={
              changeCount === 0 ||
              mutation.isPending ||
              embeddingMissingModel ||
              semesterInvalid
            }
            onClick={(e) => {
              e.preventDefault()
              mutation.mutate()
            }}
          >
            {mutation.isPending
              ? t("courses.bulk.applying")
              : t("courses.bulk.applyButton", { count: changeCount })}
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  )
}
