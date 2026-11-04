import { createFileRoute } from "@tanstack/react-router"
import { AdminUserUsagePage } from "@/components/admin/user-usage-page"

export const Route = createFileRoute("/admin/users_/$userId/usage")({
  component: () => <AdminUserUsagePage useParams={Route.useParams} />,
})
