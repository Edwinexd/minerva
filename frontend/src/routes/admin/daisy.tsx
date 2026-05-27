import { createFileRoute } from "@tanstack/react-router"
import { DaisyImportsPanel } from "@/components/admin/daisy-imports-page"

export const Route = createFileRoute("/admin/daisy")({
  component: DaisyImportsPanel,
})
