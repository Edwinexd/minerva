import { Link } from "@tanstack/react-router"
import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query"
import { useTranslation } from "react-i18next"
import { coursesQuery, unreadCountsQuery, userQuery } from "@/lib/queries"
import { api } from "@/lib/api"
import { useApiErrorMessage } from "@/lib/use-api-error"
import { useDocumentTitle } from "@/lib/use-document-title"
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

  const canCreate = user?.role === "teacher" || user?.role === "admin"

  const teacherCourses = courses?.filter(
    (c) => c.my_role === "teacher" || c.my_role === "ta",
  ) ?? []
  const studentCourses = courses?.filter((c) => c.my_role === "student") ?? []
  const hasBoth = teacherCourses.length > 0 && studentCourses.length > 0

  return (
    <div className="space-y-6">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <div>
          <h2 className="text-2xl font-bold tracking-tight">
            {user
              ? t("home.welcome", { name: user.display_name || user.eppn })
              : t("home.appName")}
          </h2>
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
        <CourseSection
          title={hasBoth ? t("home.teacherSection") : null}
          courses={teacherCourses}
          variant="teacher"
        />
      )}

      {!isLoading && studentCourses.length > 0 && (
        <CourseSection
          title={hasBoth ? t("home.studentSection") : null}
          courses={studentCourses}
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
        <h3 className="text-sm font-semibold text-muted-foreground uppercase tracking-wide">
          {title}
        </h3>
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

function CreateCourseForm({ onCreated }: { onCreated: () => void }) {
  const { t } = useTranslation("common")
  const formatError = useApiErrorMessage()
  const queryClient = useQueryClient()
  const [name, setName] = useState("")
  const [description, setDescription] = useState("")

  const mutation = useMutation({
    mutationFn: (data: { name: string; description: string | null }) =>
      api.post<Course>("/courses", data),
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
            })
          }}
        >
          <div className="space-y-2">
            <Label htmlFor="name">{t("home.create.nameLabel")}</Label>
            <Input
              id="name"
              value={name}
              onChange={(e) => setName(e.target.value)}
              placeholder={t("home.create.namePlaceholder")}
              required
            />
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
          <Button type="submit" disabled={mutation.isPending || !name}>
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
