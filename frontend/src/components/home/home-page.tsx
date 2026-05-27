import { Link } from "@tanstack/react-router"
import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query"
import { useTranslation } from "react-i18next"
import { coursesQuery, unreadCountsQuery, userQuery } from "@/lib/queries"
import { api } from "@/lib/api"
import { useApiErrorMessage } from "@/lib/use-api-error"
import { useDocumentTitle } from "@/lib/use-document-title"
import { isTeacherOrAbove } from "@/lib/roles"
import { Button } from "@/components/ui/button"
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card"
import { Badge } from "@/components/ui/badge"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"
import { Textarea } from "@/components/ui/textarea"
import { Skeleton } from "@/components/ui/skeleton"
import { useState } from "react"
import type { Course } from "@/lib/types"

export function Home() {
  const { t } = useTranslation("common")
  useDocumentTitle(t("pageTitles.home"))
  const formatError = useApiErrorMessage()
  const { data: user } = useQuery(userQuery)
  const { data: courses, isLoading, error } = useQuery(coursesQuery)
  const [showCreate, setShowCreate] = useState(false)

  const canCreate = isTeacherOrAbove(user?.role)

  const teacherCourses = courses?.filter(
    (c) => c.my_role === "teacher" || c.my_role === "ta",
  ) ?? []
  const studentCourses = courses?.filter((c) => c.my_role === "student") ?? []
  const hasBoth = teacherCourses.length > 0 && studentCourses.length > 0

  // Group teacher courses by `semester_label` so VT2026, HT2026, ...
  // each get their own header. Most teachers carry the same handful
  // of courses across multiple semesters; surfacing the term up-front
  // keeps the "current vs. last year" mental model obvious.
  const teacherBySemester = groupBySemester(teacherCourses)
  const studentBySemester = groupBySemester(studentCourses)

  return (
    <div className="space-y-6">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <div>
          <h1 className="text-2xl font-bold tracking-tight">
            {user
              ? t("home.welcome", { name: user.display_name || user.eppn })
              : t("home.appName")}
          </h1>
          <p className="text-muted-foreground mt-1">
            {user?.role === "student" ? t("home.yourCourse") : t("home.yourCourses")}
          </p>
        </div>
        {canCreate && (
          <Button onClick={() => setShowCreate(!showCreate)}>
            {showCreate ? t("actions.cancel") : t("home.newCourse")}
          </Button>
        )}
      </div>

      {showCreate && <CreateCourseForm onCreated={() => setShowCreate(false)} />}

      {error && (
        <p className="text-destructive">
          {t("home.loadFailed", { error: formatError(error) })}
        </p>
      )}

      {isLoading && (
        <div className="grid gap-4 md:grid-cols-2 lg:grid-cols-3">
          {Array.from({ length: 3 }).map((_, i) => (
            <Card key={i}>
              <CardHeader>
                <Skeleton className="h-5 w-3/4" />
                <Skeleton className="h-4 w-1/2 mt-2" />
              </CardHeader>
              <CardContent>
                <Skeleton className="h-4 w-full" />
              </CardContent>
            </Card>
          ))}
        </div>
      )}

      {!isLoading && teacherCourses.length > 0 && (
        <SemesterGroupedSections
          // Top-level role heading only when both teacher + student
          // cards are visible; otherwise the page is already a single
          // role context and the extra subheading is just noise.
          topTitle={hasBoth ? t("home.teacherSection") : null}
          groups={teacherBySemester}
          variant="teacher"
        />
      )}

      {!isLoading && studentCourses.length > 0 && (
        <SemesterGroupedSections
          topTitle={hasBoth ? t("home.studentSection") : null}
          groups={studentBySemester}
          variant="student"
          showUnread
        />
      )}

      {!isLoading && courses?.length === 0 && !showCreate && (
        <p className="text-muted-foreground">
          {canCreate ? t("home.emptyTeacher") : t("home.emptyStudent")}
        </p>
      )}
    </div>
  )
}

/**
 * Bucket a flat course list by `semester_label` and return groups
 * sorted by recency (VT2027 > HT2026 > VT2026 > ...). Courses
 * lacking a semester label fall into a sentinel "" group that
 * renders last under an "Ad-hoc" heading.
 *
 * The sort key extracts the year from `YYYY` and the season order
 * (HT > VT within a year because Aug-Dec lands later in the calendar
 * than Jan-Jun). Anything malformed (shouldn't happen post-server-
 * validation, but be defensive) sorts to the end.
 */
function semesterSortKey(label: string): number {
  if (!label) return -Infinity
  const m = label.match(/^(VT|HT)(\d{4})$/)
  if (!m) return -Infinity
  const year = parseInt(m[2], 10)
  // HT (autumn) chronologically follows VT (spring) of the same year.
  const seasonOffset = m[1] === "HT" ? 0.5 : 0
  return year + seasonOffset
}

function groupBySemester(courses: Course[]): Array<{
  label: string
  courses: Course[]
}> {
  const buckets = new Map<string, Course[]>()
  for (const c of courses) {
    const key = c.semester_label ?? ""
    if (!buckets.has(key)) buckets.set(key, [])
    buckets.get(key)!.push(c)
  }
  const entries = Array.from(buckets.entries()).map(([label, courses]) => ({
    label,
    courses,
  }))
  // Newest semester first; "Ad-hoc" (empty key) always last so it
  // doesn't hijack the visual hierarchy when most of a teacher's
  // courses are semester-tagged.
  entries.sort((a, b) => {
    if (a.label === "" && b.label !== "") return 1
    if (b.label === "" && a.label !== "") return -1
    return semesterSortKey(b.label) - semesterSortKey(a.label)
  })
  return entries
}

function SemesterGroupedSections({
  topTitle,
  groups,
  variant,
  showUnread = false,
}: {
  topTitle: string | null
  groups: Array<{ label: string; courses: Course[] }>
  variant: "teacher" | "student"
  showUnread?: boolean
}) {
  const { t } = useTranslation("common")
  // When every course in this role bucket lives in a single semester
  // (or in the "ad-hoc" bucket), the semester heading is redundant
  // with the page context. Collapse it back to a flat list so the
  // pre-Daisy presentation is preserved for teachers who haven't
  // adopted any auto-imported courses yet.
  const flat = groups.length <= 1
  if (flat) {
    return (
      <CourseSection
        title={topTitle}
        courses={groups[0]?.courses ?? []}
        variant={variant}
        showUnread={showUnread}
      />
    )
  }
  return (
    <div className="space-y-8">
      {topTitle && (
        <h2 className="text-sm font-semibold text-muted-foreground uppercase tracking-wide">
          {topTitle}
        </h2>
      )}
      {groups.map((g) => (
        <CourseSection
          key={g.label || "_adhoc"}
          title={g.label === "" ? t("home.adhocSection") : g.label}
          courses={g.courses}
          variant={variant}
          showUnread={showUnread}
        />
      ))}
    </div>
  )
}

function CourseSection({
  title,
  courses,
  variant,
  showUnread = false,
}: {
  title: string | null
  courses: Course[]
  variant: "teacher" | "student"
  /**
   * When true, fetch the per-course unread rollup and pass each
   * card its unread count. Only student tiles render the badge
   * (teachers have their own per-course dashboard with the
   * Unreviewed tab; double-surfacing on the tile would be noisy).
   */
  showUnread?: boolean
}) {
  // Cross-course rollup of conversations with unread teacher
  // notes. Only fetched when a section actually needs it; the
  // teacher tiles section skips the query entirely.
  const { data: unreadByCourse } = useQuery({
    ...unreadCountsQuery,
    enabled: showUnread,
  })
  return (
    <section className="space-y-3">
      {title && (
        <h2 className="text-sm font-semibold text-muted-foreground uppercase tracking-wide">
          {title}
        </h2>
      )}
      <div className="grid gap-4 md:grid-cols-2 lg:grid-cols-3">
        {courses.map((course) =>
          variant === "teacher" ? (
            <TeacherCourseCard key={course.id} course={course} />
          ) : (
            <StudentCourseCard
              key={course.id}
              course={course}
              unreadCount={unreadByCourse?.[course.id] ?? 0}
            />
          ),
        )}
      </div>
    </section>
  )
}

function TeacherCourseCard({ course }: { course: Course }) {
  const { t } = useTranslation("common")
  return (
    <Link to="/teacher/courses/$courseId" params={{ courseId: course.id }}>
      <Card className="hover:border-foreground/20 transition-colors cursor-pointer h-full">
        <CardHeader>
          <div className="flex items-start justify-between gap-2">
            <CardTitle className="text-lg">{course.name}</CardTitle>
            <Badge variant="outline" className="shrink-0 capitalize">
              {course.my_role}
            </Badge>
          </div>
          {course.description && (
            <CardDescription>{course.description}</CardDescription>
          )}
        </CardHeader>
        <CardContent>
          <div className="space-y-2 text-xs">
            <Badge
              variant="secondary"
              className="h-auto max-w-full whitespace-normal break-all text-left"
            >
              {course.model}
            </Badge>
            <div className="flex flex-wrap gap-2">
              <Badge variant="outline">{course.strategy}</Badge>
              <Badge variant="outline">
                {t("home.temperatureBadge", { value: course.temperature })}
              </Badge>
              <Badge variant="outline">
                {t("home.ragBadge", {
                  percent: Math.round(course.context_ratio * 100),
                })}
              </Badge>
            </div>
          </div>
        </CardContent>
      </Card>
    </Link>
  )
}

function StudentCourseCard({
  course,
  unreadCount,
}: {
  course: Course
  unreadCount: number
}) {
  const { t } = useTranslation("common")
  return (
    <Link to="/course/$courseId" params={{ courseId: course.id }}>
      <Card className="hover:border-foreground/20 transition-colors cursor-pointer h-full">
        <CardHeader>
          <div className="flex items-start justify-between gap-2">
            <CardTitle className="text-lg">{course.name}</CardTitle>
            {unreadCount > 0 && (
              // Small primary-filled badge with the unread count.
              // Significantly more discoverable than a dot inside
              // the chat sidebar (the only place a fresh teacher
              // note lived before), since this catches the user
              // before they pick which course to open.
              <Badge
                variant="default"
                className="shrink-0"
                title={t("home.unreadNotesTooltip", { count: unreadCount })}
              >
                {t("home.unreadNotesBadge", { count: unreadCount })}
              </Badge>
            )}
          </div>
          {course.description && (
            <CardDescription>{course.description}</CardDescription>
          )}
        </CardHeader>
        <CardContent>
          <p className="text-sm text-muted-foreground">{t("home.clickToChat")}</p>
        </CardContent>
      </Card>
    </Link>
  )
}

/**
 * Suggest a default `semester_label` based on today's calendar so a
 * teacher who just clicked "New Course" doesn't have to think about
 * which term they're in. Matches the same VT=Jan-Jun / HT=Jul-Dec
 * convention used by `scripts/sync_daisy_courses.py`, so manual and
 * Daisy-imported courses end up in the same buckets.
 */
function defaultSemesterLabel(today: Date = new Date()): string {
  const year = today.getFullYear()
  const month = today.getMonth() + 1 // 1-12
  return month <= 6 ? `VT${year}` : `HT${year}`
}

function CreateCourseForm({ onCreated }: { onCreated: () => void }) {
  const { t } = useTranslation("common")
  const formatError = useApiErrorMessage()
  const queryClient = useQueryClient()
  const [name, setName] = useState("")
  const [description, setDescription] = useState("")
  // Pre-populate with the current semester. Teachers prepping the
  // next term overwrite this; everyone else just hits submit.
  const [semesterLabel, setSemesterLabel] = useState(defaultSemesterLabel())

  const mutation = useMutation({
    mutationFn: (data: {
      name: string
      description: string | null
      semester_label: string
    }) => api.post<Course>("/courses", data),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["courses"] })
      onCreated()
    },
  })

  return (
    <Card>
      <CardHeader>
        <CardTitle>{t("home.create.title")}</CardTitle>
      </CardHeader>
      <CardContent>
        <form
          className="space-y-4"
          onSubmit={(e) => {
            e.preventDefault()
            mutation.mutate({
              name,
              description: description || null,
              semester_label: semesterLabel.trim().toUpperCase(),
            })
          }}
        >
          <div className="space-y-2">
            <Label htmlFor="name">
              {t("home.create.nameLabel")}{" "}
              <span className="text-destructive" aria-hidden="true">{t("forms.requiredMark")}</span>
              <span className="sr-only">{t("forms.requiredLabel")}</span>
            </Label>
            <Input
              id="name"
              value={name}
              onChange={(e) => setName(e.target.value)}
              placeholder={t("home.create.namePlaceholder")}
              required
            />
          </div>
          <div className="space-y-2">
            <Label htmlFor="semester_label">
              {t("home.create.semesterLabel")}{" "}
              <span className="text-destructive" aria-hidden="true">
                {t("forms.requiredMark")}
              </span>
              <span className="sr-only">{t("forms.requiredLabel")}</span>
            </Label>
            <Input
              id="semester_label"
              value={semesterLabel}
              onChange={(e) => setSemesterLabel(e.target.value)}
              placeholder={t("home.create.semesterPlaceholder")}
              // Client-side mirror of the backend regex. Lets the
              // browser surface the format hint before a round-trip.
              pattern="(?:VT|HT|vt|ht)\d{4}"
              title={t("home.create.semesterHint")}
              required
            />
            <p className="text-xs text-muted-foreground">
              {t("home.create.semesterHint")}
            </p>
          </div>
          <div className="space-y-2">
            <Label htmlFor="description">{t("home.create.descriptionLabel")}</Label>
            <Textarea
              id="description"
              value={description}
              onChange={(e) => setDescription(e.target.value)}
              placeholder={t("home.create.descriptionPlaceholder")}
            />
          </div>
          <Button
            type="submit"
            disabled={mutation.isPending || !name || !semesterLabel.trim()}
          >
            {mutation.isPending
              ? t("home.create.submitting")
              : t("home.create.submit")}
          </Button>
          {mutation.isError && (
            <p className="text-sm text-destructive">
              {formatError(mutation.error)}
            </p>
          )}
        </form>
      </CardContent>
    </Card>
  )
}
