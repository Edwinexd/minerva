import { createFileRoute, redirect } from "@tanstack/react-router"

export const Route = createFileRoute("/teacher/workspaces/$courseId/")({
  beforeLoad: ({ params }) => {
    throw redirect({
      to: "/teacher/workspaces/$courseId/config",
      params: { courseId: params.courseId },
    })
  },
})
