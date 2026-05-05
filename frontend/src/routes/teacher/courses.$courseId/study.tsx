import { createFileRoute } from "@tanstack/react-router"
import { StudyPage } from "@/components/teacher/study-page"

export const Route = createFileRoute("/teacher/courses/$courseId/study")({
  component: () => <StudyPage useParams={Route.useParams} />,
})
