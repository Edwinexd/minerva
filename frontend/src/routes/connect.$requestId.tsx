import { createFileRoute, redirect } from "@tanstack/react-router"

// Old address, kept as a redirect for an authorization started before
// the move.
export const Route = createFileRoute("/connect/$requestId")({
  beforeLoad: ({ params }) => {
    throw redirect({
      to: "/oauth/requests/$requestId",
      params: { requestId: params.requestId },
      replace: true,
    })
  },
})
