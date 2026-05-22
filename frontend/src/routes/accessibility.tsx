import { createFileRoute } from "@tanstack/react-router"
import { AccessibilityPage } from "@/components/accessibility"

export const Route = createFileRoute("/accessibility")({
  component: AccessibilityPage,
})
