import { createFileRoute, redirect } from "@tanstack/react-router"

// Old address of the workspace settings pages, kept as a redirect for
// bookmarks and links already sent: /teacher/courses/{id}/{tab}.
export const Route = createFileRoute("/teacher/courses/$courseId/$")({
  beforeLoad: ({ params }) => {
    throw redirect({
      href: `/teacher/workspaces/${params.courseId}/${params._splat ?? ""}`,
      replace: true,
    })
  },
})
