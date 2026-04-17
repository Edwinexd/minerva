import { createFileRoute } from "@tanstack/react-router"
import { DataHandlingPage } from "@/components/data-handling"

export const Route = createFileRoute("/data-handling")({
  component: DataHandlingPage,
})
