import { createFileRoute, redirect } from "@tanstack/react-router"

/// Kept as a redirect: the guide moved into the teacher portal, and this
/// path is what older links and bookmarks point at.
export const Route = createFileRoute("/teacher-help")({
  beforeLoad: () => {
    throw redirect({ to: "/teacher/guide" })
  },
})
