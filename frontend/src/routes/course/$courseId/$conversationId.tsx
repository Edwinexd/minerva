import { createFileRoute, redirect } from "@tanstack/react-router"

// Old address, kept as a redirect (shared links to a conversation). A
// credit transfer case moves on to /cases from the conversations page.
export const Route = createFileRoute("/course/$courseId/$conversationId")({
  beforeLoad: ({ params }) => {
    throw redirect({
      to: "/workspaces/$workspaceId/conversations/$conversationId",
      params: { workspaceId: params.courseId, conversationId: params.conversationId },
      replace: true,
    })
  },
})
