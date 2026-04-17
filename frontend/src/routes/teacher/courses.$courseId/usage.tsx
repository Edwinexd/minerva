import { createFileRoute } from "@tanstack/react-router"
import { UsagePage } from "@/components/teacher/usage-page"

export const Route = createFileRoute("/teacher/courses/$courseId/usage")({
  component: () => <UsagePage useParams={Route.useParams} />,
})
