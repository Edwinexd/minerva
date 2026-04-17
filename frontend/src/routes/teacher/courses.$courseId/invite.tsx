import { createFileRoute } from "@tanstack/react-router"
import { InvitePage } from "@/components/teacher/invite-page"

export const Route = createFileRoute("/teacher/courses/$courseId/invite")({
  component: () => <InvitePage useParams={Route.useParams} />,
})
