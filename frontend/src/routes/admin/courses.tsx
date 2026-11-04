import { createFileRoute, redirect } from "@tanstack/react-router"

// Old address, kept as a redirect.
export const Route = createFileRoute("/admin/courses")({
  beforeLoad: () => {
    throw redirect({ to: "/admin/workspaces", replace: true })
  },
})
