import { createFileRoute } from "@tanstack/react-router"
import { ChatRouteComponent } from "@/components/chat/chat-page"

export const Route = createFileRoute("/workspaces/$workspaceId/cases/$caseId")({
  component: () => (
    <ChatRouteComponent
      useParams={() => {
        const { workspaceId, caseId } = Route.useParams()
        return { courseId: workspaceId, conversationId: caseId }
      }}
    />
  ),
})
