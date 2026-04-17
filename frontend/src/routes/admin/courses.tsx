import { createFileRoute } from "@tanstack/react-router"
import { CourseManagementPanel } from "@/components/admin/courses-page"

export const Route = createFileRoute("/admin/courses")({
  component: CourseManagementPanel,
})
