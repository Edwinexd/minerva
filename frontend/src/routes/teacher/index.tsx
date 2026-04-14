import { createFileRoute, redirect } from "@tanstack/react-router"

export const Route = createFileRoute("/teacher/")({
  beforeLoad: () => {
    throw redirect({ to: "/" })
  },
})
