import { createFileRoute } from "@tanstack/react-router"
import { ConfigPage } from "@/components/teacher/config-page"

export const Route = createFileRoute("/teacher/workspaces/$courseId/config")({
  component: () => <ConfigPage useParams={Route.useParams} />,
})
