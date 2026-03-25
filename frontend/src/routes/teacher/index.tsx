import { createFileRoute, Link } from "@tanstack/react-router"
import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query"
import { coursesQuery } from "@/lib/queries"
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

export const Route = createFileRoute("/teacher/")({
  component: TeacherDashboard,
})

function TeacherDashboard() {
  const { data: courses, isLoading, error } = useQuery(coursesQuery)
  const [showCreate, setShowCreate] = useState(false)

  return (
    <div className="space-y-6">
      <div className="flex items-center justify-between">
        <h2 className="text-2xl font-bold tracking-tight">Your Courses</h2>
        <Button onClick={() => setShowCreate(!showCreate)}>
          {showCreate ? "Cancel" : "New Course"}
        </Button>
      </div>

      {showCreate && <CreateCourseForm onCreated={() => setShowCreate(false)} />}

      {error && (
        <p className="text-destructive">Failed to load courses: {error.message}</p>
      )}

      <div className="grid gap-4 md:grid-cols-2 lg:grid-cols-3">
        {isLoading &&
          Array.from({ length: 3 }).map((_, i) => (
            <Card key={i}>
              <CardHeader>
                <Skeleton className="h-5 w-3/4" />
                <Skeleton className="h-4 w-1/2 mt-2" />
              </CardHeader>
              <CardContent>
                <div className="flex gap-2">
                  <Skeleton className="h-5 w-20" />
                  <Skeleton className="h-5 w-16" />
                  <Skeleton className="h-5 w-16" />
                </div>
              </CardContent>
            </Card>
          ))}
        {courses?.length === 0 && !showCreate && (
          <p className="text-muted-foreground col-span-full">
            No courses yet. Create your first course to get started.
          </p>
        )}
        {courses?.map((course) => (
          <CourseCard key={course.id} course={course} />
        ))}
      </div>
    </div>
  )
}

function CourseCard({ course }: { course: Course }) {
  return (
    <Link to="/teacher/courses/$courseId" params={{ courseId: course.id }}>
      <Card className="hover:border-foreground/20 transition-colors cursor-pointer">
        <CardHeader>
          <CardTitle className="text-lg">{course.name}</CardTitle>
          {course.description && (
            <CardDescription>{course.description}</CardDescription>
          )}
        </CardHeader>
        <CardContent>
          <div className="flex gap-2 text-xs">
            <Badge variant="secondary">{course.model}</Badge>
            <Badge variant="outline">{course.strategy}</Badge>
            <Badge variant="outline">T={course.temperature}</Badge>
            <Badge variant="outline">
              {Math.round(course.context_ratio * 100)}% RAG
            </Badge>
          </div>
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
