import { createFileRoute } from "@tanstack/react-router"
import { LtiBindPage } from "@/components/lti-bind-page"

export const Route = createFileRoute("/lti-bind")({
  component: LtiBindPage,
})
