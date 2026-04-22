import { createFileRoute } from "@tanstack/react-router"
import { LtiPlatformsPanel } from "@/components/admin/lti-platforms-page"

export const Route = createFileRoute("/admin/lti")({
  component: LtiPlatformsPanel,
})
