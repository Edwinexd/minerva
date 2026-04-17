import { createFileRoute } from "@tanstack/react-router"
import { CourseEditPage } from "@/components/teacher/course-edit-page"

export const Route = createFileRoute("/teacher/courses/$courseId")({
  component: () => <CourseEditPage useParams={Route.useParams} />,
})
