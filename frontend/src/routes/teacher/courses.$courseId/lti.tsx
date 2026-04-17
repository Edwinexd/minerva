import { createFileRoute } from "@tanstack/react-router"
import { LtiPage } from "@/components/teacher/lti-page"

export const Route = createFileRoute("/teacher/courses/$courseId/lti")({
  component: () => <LtiPage useParams={Route.useParams} />,
})
