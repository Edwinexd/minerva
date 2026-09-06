import { createFileRoute } from "@tanstack/react-router"
import { TeacherPortalLayout } from "@/components/teacher/portal-layout"

export const Route = createFileRoute("/teacher/_portal")({
  component: TeacherPortalLayout,
})
