import { createFileRoute } from "@tanstack/react-router"
import { AdminUserUsagePage } from "@/components/admin/user-usage-page"

export const Route = createFileRoute("/admin/usage/$userId")({
  component: () => <AdminUserUsagePage useParams={Route.useParams} />,
})
