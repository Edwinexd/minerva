import { createFileRoute } from "@tanstack/react-router"
import { AdminDefaultsPanel } from "@/components/admin/defaults-page"

export const Route = createFileRoute("/admin/defaults")({
  component: AdminDefaultsPanel,
})
