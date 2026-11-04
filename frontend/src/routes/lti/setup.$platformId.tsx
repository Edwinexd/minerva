import { createFileRoute, redirect } from "@tanstack/react-router"

// Old address, kept as a redirect with its query (`name`, `issuer`) for
// a dynamic registration started before the move.
export const Route = createFileRoute("/lti/setup/$platformId")({
  beforeLoad: ({ params, location }) => {
    throw redirect({
      href: `/lti/platforms/${params.platformId}/setup${location.searchStr}`,
      replace: true,
    })
  },
})
