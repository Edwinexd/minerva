import { createFileRoute } from "@tanstack/react-router"
import { RagPage } from "@/components/teacher/rag-page"

export const Route = createFileRoute("/teacher/workspaces/$courseId/rag")({
  component: () => <RagPage useParams={Route.useParams} />,
})
