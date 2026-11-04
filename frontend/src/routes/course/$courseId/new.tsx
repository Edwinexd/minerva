import { createFileRoute, redirect } from "@tanstack/react-router"

// Old address, kept as a redirect. A credit transfer workspace moves its
// new case on to /cases from the conversations page.
export const Route = createFileRoute("/course/$courseId/new")({
  beforeLoad: ({ params }) => {
    throw redirect({
      to: "/workspaces/$workspaceId/conversations/new",
      params: { workspaceId: params.courseId },
      replace: true,
    })
  },
})
