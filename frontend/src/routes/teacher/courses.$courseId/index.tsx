import { createFileRoute, redirect } from "@tanstack/react-router"

export const Route = createFileRoute("/teacher/courses/$courseId/")({
  beforeLoad: ({ params }) => {
    throw redirect({
      to: `/teacher/courses/${params.courseId}/config`,
    } as any)
  },
})
