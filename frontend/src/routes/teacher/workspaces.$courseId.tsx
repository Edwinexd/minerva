import { createFileRoute } from "@tanstack/react-router"
import { CourseEditPage } from "@/components/teacher/course-edit-page"

export const Route = createFileRoute("/teacher/workspaces/$courseId")({
  component: () => <CourseEditPage useParams={Route.useParams} />,
})
