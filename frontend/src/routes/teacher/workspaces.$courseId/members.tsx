import { createFileRoute } from "@tanstack/react-router"
import { MembersPage } from "@/components/teacher/members-page"

export const Route = createFileRoute("/teacher/workspaces/$courseId/members")({
  component: () => <MembersPage useParams={Route.useParams} />,
})
