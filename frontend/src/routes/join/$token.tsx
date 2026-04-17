import { createFileRoute } from "@tanstack/react-router"
import { JoinPage } from "@/components/join/join-page"

export const Route = createFileRoute("/join/$token")({
  component: () => <JoinPage useParams={Route.useParams} />,
})
