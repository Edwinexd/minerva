import { createFileRoute, redirect } from "@tanstack/react-router"

// Old address of a new credit transfer chat, kept as a redirect.
export const Route = createFileRoute("/workspaces/$workspaceId/cases/new")({
  beforeLoad: ({ params }) => {
    throw redirect({
      to: "/workspaces/$workspaceId/conversations/new",
      params: { workspaceId: params.workspaceId },
      replace: true,
    })
  },
})
