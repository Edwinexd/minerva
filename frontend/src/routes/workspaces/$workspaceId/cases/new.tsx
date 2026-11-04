import { createFileRoute } from "@tanstack/react-router"
import { NewChatRouteComponent } from "@/components/chat/chat-page"

// A credit transfer case is a conversation; same page, its own name.
export const Route = createFileRoute("/workspaces/$workspaceId/cases/new")({
  component: () => (
    <NewChatRouteComponent useParams={() => ({ courseId: Route.useParams().workspaceId })} />
  ),
})
