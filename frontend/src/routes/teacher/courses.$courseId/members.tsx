import { createFileRoute } from "@tanstack/react-router"
import { MembersPage } from "@/components/teacher/members-page"

export const Route = createFileRoute("/teacher/courses/$courseId/members")({
  component: () => <MembersPage useParams={Route.useParams} />,
})
