import { createFileRoute, redirect } from "@tanstack/react-router"
import { userQuery } from "@/lib/queries"
import { isTeacherOrAbove } from "@/lib/roles"

export const Route = createFileRoute("/teacher/")({
  beforeLoad: async ({ context }) => {
    // `/teacher` is the portal's entry point for teachers; anyone else
    // has nothing to see there (its data is owner-scoped and the backend
    // 403s), so they land back on their course list.
    const user = await context.queryClient.ensureQueryData(userQuery)
    throw redirect({ to: isTeacherOrAbove(user?.role) ? "/teacher/usage" : "/" })
  },
})
