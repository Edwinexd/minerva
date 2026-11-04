import { createFileRoute } from "@tanstack/react-router"
import { ApiKeysPage } from "@/components/teacher/api-keys-page"

export const Route = createFileRoute("/teacher/workspaces/$courseId/api-keys")({
  component: () => <ApiKeysPage useParams={Route.useParams} />,
})
