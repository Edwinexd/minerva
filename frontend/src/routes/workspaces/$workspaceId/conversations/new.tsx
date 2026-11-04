import { createFileRoute } from "@tanstack/react-router"
import { NewChatRouteComponent } from "@/components/chat/chat-page"

export const Route = createFileRoute("/workspaces/$workspaceId/conversations/new")({
  component: () => (
    <NewChatRouteComponent useParams={() => ({ courseId: Route.useParams().workspaceId })} />
  ),
})
