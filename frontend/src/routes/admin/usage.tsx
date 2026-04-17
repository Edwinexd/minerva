import { createFileRoute } from "@tanstack/react-router"
import { PlatformUsagePanel } from "@/components/admin/usage-page"

export const Route = createFileRoute("/admin/usage")({
  component: PlatformUsagePanel,
})
