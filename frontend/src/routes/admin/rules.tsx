import { createFileRoute } from "@tanstack/react-router"
import { RoleRulesPanel } from "@/components/admin/rules-page"

export const Route = createFileRoute("/admin/rules")({
  component: RoleRulesPanel,
})
