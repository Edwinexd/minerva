import { createFileRoute } from "@tanstack/react-router"
import { ConversationsPage } from "@/components/teacher/conversations-page"

export const Route = createFileRoute("/teacher/courses/$courseId/conversations")({
  component: () => <ConversationsPage useParams={Route.useParams} />,
})
