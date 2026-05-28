import { createFileRoute } from "@tanstack/react-router"
import { TeacherHelpPage } from "@/components/teacher-help-page"

export const Route = createFileRoute("/teacher-help")({
  component: TeacherHelpPage,
})
