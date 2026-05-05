import { createFileRoute } from "@tanstack/react-router"
import { StudyPage } from "@/components/study/study-page"

export const Route = createFileRoute("/course/$courseId/study")({
  component: () => <StudyPage useParams={Route.useParams} />,
})
