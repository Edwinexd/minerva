import { createFileRoute } from "@tanstack/react-router"
import { AdminLayout } from "@/components/admin/admin-layout"

export const Route = createFileRoute("/admin")({
  component: AdminLayout,
})
