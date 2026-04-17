import { createRootRouteWithContext } from "@tanstack/react-router"
import type { QueryClient } from "@tanstack/react-query"
import { RootLayout } from "@/components/root-layout"

interface RouterContext {
  queryClient: QueryClient
}

export const Route = createRootRouteWithContext<RouterContext>()({
  component: RootLayout,
})
