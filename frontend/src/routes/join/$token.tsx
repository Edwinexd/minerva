import { createFileRoute, redirect } from "@tanstack/react-router"

// Old invite address, kept as a redirect: join links already handed out
// (course pages, emails) use it.
export const Route = createFileRoute("/join/$token")({
  beforeLoad: ({ params }) => {
    throw redirect({ to: "/invites/$token", params: { token: params.token }, replace: true })
  },
})
