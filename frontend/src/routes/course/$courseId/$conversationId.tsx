import { createFileRoute } from "@tanstack/react-router"
import { ChatRouteComponent } from "@/components/chat/chat-page"

export const Route = createFileRoute("/course/$courseId/$conversationId")({
  component: () => <ChatRouteComponent useParams={Route.useParams} />,
})
