import { createFileRoute } from "@tanstack/react-router"
import { LtiPage } from "@/components/teacher/lti-page"

export const Route = createFileRoute("/teacher/workspaces/$courseId/lti")({
  component: () => <LtiPage useParams={Route.useParams} />,
})
