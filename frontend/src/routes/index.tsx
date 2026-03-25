import { createFileRoute, Link, useNavigate } from "@tanstack/react-router"
import { useQuery } from "@tanstack/react-query"
import { coursesQuery, userQuery } from "@/lib/queries"
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card"
import { Skeleton } from "@/components/ui/skeleton"
import { useEffect } from "react"

export const Route = createFileRoute("/")({
  component: Home,
})

function Home() {
  const { data: user } = useQuery(userQuery)
  const navigate = useNavigate()

  // Teachers and admins go straight to dashboard
  useEffect(() => {
    if (user && (user.role === "teacher" || user.role === "admin")) {
      navigate({ to: "/teacher", replace: true })
    }
  }, [user, navigate])

  // Students see their courses
  return <StudentHome />
}

function StudentHome() {
  const { data: user } = useQuery(userQuery)
  const { data: courses, isLoading, error } = useQuery(coursesQuery)

  return (
    <div className="space-y-6">
      <div>
        <h2 className="text-2xl font-bold tracking-tight">
          {user ? `Welcome, ${user.display_name || user.eppn}` : "Minerva"}
        </h2>
        <p className="text-muted-foreground mt-1">Your courses</p>
      </div>

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
                <Skeleton className="h-4 w-full" />
              </CardContent>
            </Card>
          ))}
        {courses?.map((course) => (
          <Link key={course.id} to="/course/$courseId" params={{ courseId: course.id }}>
            <Card className="hover:border-foreground/20 transition-colors cursor-pointer">
              <CardHeader>
                <CardTitle className="text-lg">{course.name}</CardTitle>
                {course.description && (
                  <CardDescription>{course.description}</CardDescription>
                )}
              </CardHeader>
              <CardContent>
                <p className="text-sm text-muted-foreground">
                  Click to start chatting
                </p>
              </CardContent>
            </Card>
          </Link>
        ))}
      </div>

      {!isLoading && courses?.length === 0 && (
        <p className="text-muted-foreground">
          You haven't been added to any courses yet. Ask your teacher for an invite link.
        </p>
      )}
    </div>
  )
}
