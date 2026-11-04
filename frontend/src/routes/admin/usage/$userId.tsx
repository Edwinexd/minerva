import { createFileRoute, redirect } from "@tanstack/react-router"

// Old address, kept as a redirect: a user's usage lives under the user.
export const Route = createFileRoute("/admin/usage/$userId")({
  beforeLoad: ({ params }) => {
    throw redirect({
      to: "/admin/users/$userId/usage",
      params: { userId: params.userId },
      replace: true,
    })
  },
})
