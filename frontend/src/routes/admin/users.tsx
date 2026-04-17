import { createFileRoute } from "@tanstack/react-router"
import { UserManagementPanel } from "@/components/admin/users-page"

export const Route = createFileRoute("/admin/users")({
  component: UserManagementPanel,
})
