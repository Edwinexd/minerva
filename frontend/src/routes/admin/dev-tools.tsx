import { createFileRoute } from "@tanstack/react-router"
import { DevToolsPanel } from "@/components/admin/dev-tools-page"

export const Route = createFileRoute("/admin/dev-tools")({
  component: DevToolsPanel,
})
