import { createFileRoute, redirect } from "@tanstack/react-router"

// Old address, kept as a redirect: LMS popups opened before the move.
export const Route = createFileRoute("/admin/lti-approve/$platformId")({
  beforeLoad: ({ params }) => {
    throw redirect({
      to: "/admin/lti/platforms/$platformId/approve",
      params: { platformId: params.platformId },
      replace: true,
    })
  },
})
