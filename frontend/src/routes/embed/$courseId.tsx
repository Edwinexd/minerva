import { createFileRoute, redirect } from "@tanstack/react-router"

// Old embed address, kept as a redirect with its query (the launch token)
// for an LTI frame opened before the move.
export const Route = createFileRoute("/embed/$courseId")({
  beforeLoad: ({ params, location }) => {
    throw redirect({
      href: `/embed/courses/${params.courseId}${location.searchStr}`,
      replace: true,
    })
  },
})
