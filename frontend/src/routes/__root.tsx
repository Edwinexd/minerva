import { createRootRouteWithContext } from "@tanstack/react-router"
import type { QueryClient } from "@tanstack/react-query"
import { RootLayout } from "@/components/root-layout"
import { EmbedNavProvider } from "@/components/embed-nav-provider"

interface RouterContext {
  queryClient: QueryClient
}

// The provider sits above RootLayout so the header can read the embed page's
// selected conversation; RootLayout itself cannot consume a context it renders.
export const Route = createRootRouteWithContext<RouterContext>()({
  component: () => (
    <EmbedNavProvider>
      <RootLayout />
    </EmbedNavProvider>
  ),
})
