import { createFileRoute } from "@tanstack/react-router"
import { AdminStudyPanel } from "@/components/admin/study-page"

export const Route = createFileRoute("/admin/study")({
  component: AdminStudyPanel,
})
