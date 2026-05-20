import { createFileRoute, redirect } from "@tanstack/react-router"
import { userQuery } from "@/lib/queries"

export const Route = createFileRoute("/admin/")({
  beforeLoad: async ({ context }) => {
    // Integrators can't see the usage tab (its data is admin-only); land them
    // on the first tab they can actually use. Admins keep the usage default.
    const user = await context.queryClient.ensureQueryData(userQuery)
    throw redirect({
      to: user?.role === "integrator" ? "/admin/lti" : "/admin/usage",
    })
  },
})
