import { createFileRoute } from "@tanstack/react-router"
import { CanvasPage } from "@/components/teacher/canvas-page"

export const Route = createFileRoute("/teacher/courses/$courseId/canvas")({
  component: () => <CanvasPage useParams={Route.useParams} />,
})
