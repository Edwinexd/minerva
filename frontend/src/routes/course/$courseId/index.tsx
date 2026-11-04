import { createFileRoute, redirect } from "@tanstack/react-router"

// Old address, kept as a redirect: join links, bookmarks and emails
// still point here. Workspaces live under /workspaces now.
export const Route = createFileRoute("/course/$courseId/")({
  beforeLoad: ({ params }) => {
    throw redirect({
      to: "/workspaces/$workspaceId",
      params: { workspaceId: params.courseId },
      replace: true,
    })
  },
})
