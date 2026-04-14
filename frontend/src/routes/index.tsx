import { createFileRoute, Link } from "@tanstack/react-router"
import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query"
import { coursesQuery, userQuery } from "@/lib/queries"
import { api } from "@/lib/api"
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

export const Route = createFileRoute("/")({
  component: Home,
})

function Home() {
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
            {user ? `Welcome, ${user.display_name || user.eppn}` : "Minerva"}
          </h2>
          <p className="text-muted-foreground mt-1">Your courses</p>
        </div>
        {canCreate && (
          <Button onClick={() => setShowCreate(!showCreate)}>
            {showCreate ? "Cancel" : "New Course"}
          </Button>
        )}
      </div>

      {showCreate && <CreateCourseForm onCreated={() => setShowCreate(false)} />}

      {error && (
        <p className="text-destructive">Failed to load courses: {error.message}</p>
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
          title={hasBoth ? "Courses you teach" : null}
          courses={teacherCourses}
          variant="teacher"
        />
      )}

      {!isLoading && studentCourses.length > 0 && (
        <CourseSection
          title={hasBoth ? "Courses you're enrolled in" : null}
          courses={studentCourses}
          variant="student"
        />
      )}

      {!isLoading && courses?.length === 0 && !showCreate && (
        <p className="text-muted-foreground">
          {canCreate
            ? "No courses yet. Create your first course to get started."
            : "You haven't been added to any courses yet. Ask your teacher for an invite link."}
        </p>
      )}
    </div>
  )
}

function CourseSection({
  title,
  courses,
  variant,
}: {
  title: string | null
  courses: Course[]
  variant: "teacher" | "student"
}) {
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
            <StudentCourseCard key={course.id} course={course} />
          ),
        )}
      </div>
    </section>
  )
}

function TeacherCourseCard({ course }: { course: Course }) {
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
              <Badge variant="outline">T={course.temperature}</Badge>
              <Badge variant="outline">
                {Math.round(course.context_ratio * 100)}% RAG
              </Badge>
            </div>
          </div>
        </CardContent>
      </Card>
    </Link>
  )
}

function StudentCourseCard({ course }: { course: Course }) {
  return (
    <Link to="/course/$courseId" params={{ courseId: course.id }}>
      <Card className="hover:border-foreground/20 transition-colors cursor-pointer h-full">
        <CardHeader>
          <CardTitle className="text-lg">{course.name}</CardTitle>
          {course.description && (
            <CardDescription>{course.description}</CardDescription>
          )}
        </CardHeader>
        <CardContent>
          <p className="text-sm text-muted-foreground">Click to start chatting</p>
        </CardContent>
      </Card>
    </Link>
  )
}

function CreateCourseForm({ onCreated }: { onCreated: () => void }) {
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
        <CardTitle>Create Course</CardTitle>
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
            <Label htmlFor="name">Course Name</Label>
            <Input
              id="name"
              value={name}
              onChange={(e) => setName(e.target.value)}
              placeholder="e.g. Prog2 Spring 2026"
              required
            />
          </div>
          <div className="space-y-2">
            <Label htmlFor="description">Description</Label>
            <Textarea
              id="description"
              value={description}
              onChange={(e) => setDescription(e.target.value)}
              placeholder="Optional description"
            />
          </div>
          <Button type="submit" disabled={mutation.isPending || !name}>
            {mutation.isPending ? "Creating..." : "Create"}
          </Button>
          {mutation.isError && (
            <p className="text-sm text-destructive">{mutation.error.message}</p>
          )}
        </form>
      </CardContent>
    </Card>
  )
}
