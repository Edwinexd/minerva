import { createFileRoute } from "@tanstack/react-router"
import { EmbedPage } from "@/components/embed/embed-page"

export const Route = createFileRoute("/embed/$courseId")({
  component: () => <EmbedPage useParams={Route.useParams} />,
})
