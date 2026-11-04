import { createFileRoute } from "@tanstack/react-router"
import { ChatRouteComponent } from "@/components/chat/chat-page"

export const Route = createFileRoute("/workspaces/$workspaceId/conversations/$conversationId")({
  component: () => (
    <ChatRouteComponent
      useParams={() => {
        const { workspaceId, conversationId } = Route.useParams()
        return { courseId: workspaceId, conversationId }
      }}
    />
  ),
})
