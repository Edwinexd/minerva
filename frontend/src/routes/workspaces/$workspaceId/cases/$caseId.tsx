import { createFileRoute, redirect } from "@tanstack/react-router"

// Old address of a credit transfer chat, kept as a redirect.
export const Route = createFileRoute("/workspaces/$workspaceId/cases/$caseId")({
  beforeLoad: ({ params }) => {
    throw redirect({
      to: "/workspaces/$workspaceId/conversations/$conversationId",
      params: { workspaceId: params.workspaceId, conversationId: params.caseId },
      replace: true,
    })
  },
})
