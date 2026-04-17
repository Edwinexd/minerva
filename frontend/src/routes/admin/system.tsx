import { createFileRoute } from "@tanstack/react-router"
import { SystemPanel } from "@/components/admin/system-page"

export const Route = createFileRoute("/admin/system")({
  component: SystemPanel,
})
