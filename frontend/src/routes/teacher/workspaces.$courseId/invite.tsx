import { createFileRoute } from "@tanstack/react-router"
import { InvitePage } from "@/components/teacher/invite-page"

export const Route = createFileRoute("/teacher/workspaces/$courseId/invite")({
  component: () => <InvitePage useParams={Route.useParams} />,
})
