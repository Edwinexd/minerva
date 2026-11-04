import { createFileRoute } from "@tanstack/react-router"
import { AssistantsPage } from "@/components/teacher/assistants-page"

export const Route = createFileRoute("/teacher/_portal/assistants")({
  component: AssistantsPage,
})
