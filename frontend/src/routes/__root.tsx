import { createRootRouteWithContext, Link, Outlet } from "@tanstack/react-router"
import { useQuery, useQueryClient } from "@tanstack/react-query"
import type { QueryClient } from "@tanstack/react-query"
import { userQuery } from "@/lib/queries"
import { api } from "@/lib/api"
import { useState, useEffect } from "react"

interface RouterContext {
  queryClient: QueryClient
}

interface DevConfig {
  dev_mode: boolean
  users?: { eppn: string; label: string }[]
}

export const Route = createRootRouteWithContext<RouterContext>()({
  component: RootLayout,
})

function RootLayout() {
  const { data: user } = useQuery(userQuery)
  const { data: devConfig } = useQuery({
    queryKey: ["dev", "config"],
    queryFn: () => api.get<DevConfig>("/dev/config"),
    staleTime: Infinity,
  })

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
            {devConfig?.dev_mode && devConfig.users ? (
              <DevUserSwitcher users={devConfig.users} />
            ) : (
              user && (
                <span className="text-muted-foreground">
                  {user.display_name || user.eppn}
                </span>
              )
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

function DevUserSwitcher({
  users,
}: {
  users: { eppn: string; label: string }[]
}) {
  const queryClient = useQueryClient()
  const [selected, setSelected] = useState(() => {
    return localStorage.getItem("minerva-dev-user") || users[0]?.eppn || ""
  })

  // Set the header for all future requests
  useEffect(() => {
    localStorage.setItem("minerva-dev-user", selected)
    // Invalidate all queries to refetch with new user
    queryClient.invalidateQueries()
  }, [selected, queryClient])

  return (
    <div className="flex items-center gap-2">
      <span className="text-xs text-muted-foreground font-mono bg-muted px-1.5 py-0.5 rounded">DEV</span>
      <select
        value={selected}
        onChange={(e) => setSelected(e.target.value)}
        className="border rounded px-2 py-1 text-sm bg-background"
      >
        {users.map((u) => (
          <option key={u.eppn} value={u.eppn}>
            {u.label} ({u.eppn})
          </option>
        ))}
      </select>
    </div>
  )
}
