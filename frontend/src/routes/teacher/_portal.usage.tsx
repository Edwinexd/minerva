import { createFileRoute } from "@tanstack/react-router"
import { OwnerUsagePage } from "@/components/teacher/owner-usage-page"

export const Route = createFileRoute("/teacher/_portal/usage")({
  component: OwnerUsagePage,
})
