import { createFileRoute } from "@tanstack/react-router"
import { ConfigPage } from "@/components/teacher/config-page"

export const Route = createFileRoute("/teacher/courses/$courseId/config")({
  component: () => <ConfigPage useParams={Route.useParams} />,
})
