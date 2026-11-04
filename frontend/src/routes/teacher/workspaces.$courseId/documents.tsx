import { createFileRoute } from "@tanstack/react-router"
import { DocumentsPage } from "@/components/teacher/documents-page"

export const Route = createFileRoute("/teacher/workspaces/$courseId/documents")({
  component: () => <DocumentsPage useParams={Route.useParams} />,
})
