import { createFileRoute } from "@tanstack/react-router"
import { ConnectPage } from "@/components/connect-page"

export const Route = createFileRoute("/connect/$requestId")({
  component: function ConnectRoute() {
    const { requestId } = Route.useParams()
    return <ConnectPage requestId={requestId} />
  },
})
