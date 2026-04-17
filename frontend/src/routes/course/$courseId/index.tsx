import { createFileRoute } from "@tanstack/react-router"
import { CourseIndex } from "@/components/course/course-index"

export const Route = createFileRoute("/course/$courseId/")({
  component: () => <CourseIndex useParams={Route.useParams} />,
})
