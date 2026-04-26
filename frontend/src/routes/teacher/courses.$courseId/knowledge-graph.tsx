import { createFileRoute } from "@tanstack/react-router"
import { KnowledgeGraphPage } from "@/components/teacher/knowledge-graph-page"

export const Route = createFileRoute(
  "/teacher/courses/$courseId/knowledge-graph",
)({
  component: () => <KnowledgeGraphPage useParams={Route.useParams} />,
})
