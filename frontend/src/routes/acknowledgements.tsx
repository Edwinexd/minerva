import { createFileRoute } from "@tanstack/react-router"
import { AcknowledgementsPage } from "@/components/acknowledgements"

export const Route = createFileRoute("/acknowledgements")({
  component: AcknowledgementsPage,
})
