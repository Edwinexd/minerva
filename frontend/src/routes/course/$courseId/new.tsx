import { createFileRoute } from "@tanstack/react-router"
import { NewChatRouteComponent } from "@/components/chat/chat-page"

export const Route = createFileRoute("/course/$courseId/new")({
  component: () => <NewChatRouteComponent useParams={Route.useParams} />,
})
