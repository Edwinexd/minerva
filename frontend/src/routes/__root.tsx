import { createRootRouteWithContext, Link, Outlet } from "@tanstack/react-router"
import { useQuery } from "@tanstack/react-query"
import type { QueryClient } from "@tanstack/react-query"
import { userQuery } from "@/lib/queries"

interface RouterContext {
  queryClient: QueryClient
}

export const Route = createRootRouteWithContext<RouterContext>()({
  component: RootLayout,
})

function RootLayout() {
  const { data: user } = useQuery(userQuery)

  return (
    <div className="min-h-screen bg-background text-foreground">
      <header className="border-b px-6 py-4">
        <div className="flex items-center justify-between max-w-7xl mx-auto">
          <Link to="/" className="text-xl font-bold tracking-tight hover:opacity-80">
            Minerva
          </Link>
          <nav className="flex items-center gap-4 text-sm">
            {user && (user.role === "teacher" || user.role === "admin") && (
              <Link
                to="/teacher"
                className="text-muted-foreground hover:text-foreground"
              >
                Dashboard
              </Link>
            )}
            {user && (
              <span className="text-muted-foreground">
                {user.display_name || user.eppn}
              </span>
            )}
          </nav>
        </div>
      </header>
      <main className="max-w-7xl mx-auto px-6 py-8">
        <Outlet />
      </main>
    </div>
  )
}
