import { createFileRoute } from "@tanstack/react-router"
import { IntegrationKeysPanel } from "@/components/admin/integration-keys-page"

export const Route = createFileRoute("/admin/integrations")({
  component: IntegrationKeysPanel,
})
