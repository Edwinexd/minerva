import { createFileRoute } from "@tanstack/react-router"
import { PlayDesignationsPage } from "@/components/teacher/play-designations-page"

export const Route = createFileRoute(
  "/teacher/courses/$courseId/play-designations",
)({
  component: () => <PlayDesignationsPage useParams={Route.useParams} />,
})
