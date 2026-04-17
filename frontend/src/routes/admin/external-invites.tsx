import { createFileRoute } from "@tanstack/react-router"
import { ExternalInvitesPanel } from "@/components/admin/external-invites-page"

export const Route = createFileRoute("/admin/external-invites")({
  component: ExternalInvitesPanel,
})
