import { createRootRouteWithContext, Link, Outlet } from "@tanstack/react-router"
import { useQuery, useQueryClient } from "@tanstack/react-query"
import type { QueryClient } from "@tanstack/react-query"
import { userQuery } from "@/lib/queries"
import { api } from "@/lib/api"
import { ExternalLink } from "lucide-react"
import { useState, useEffect, useMemo } from "react"

interface RouterContext {
  queryClient: QueryClient
}

interface DevConfig {
  dev_mode: boolean
  users?: { eppn: string; label: string }[]
}

interface EmbedMe {
  id: string
  eppn: string
  display_name: string | null
  lti_client_id: string | null
}

export const Route = createRootRouteWithContext<RouterContext>()({
  component: RootLayout,
})

function RootLayout() {
  const isEmbed = window.location.pathname.startsWith("/embed/")

  const embedParams = useMemo(() => {
    if (!isEmbed) return null
    const params = new URLSearchParams(window.location.search)
    return {
      token: params.get("token"),
      ltiClientId: params.get("lti_client_id"),
      courseId: window.location.pathname.split("/embed/")[1]?.split("?")[0],
    }
  }, [isEmbed])

  const { data: user } = useQuery({ ...userQuery, enabled: !isEmbed })
  const { data: devConfig } = useQuery({
    queryKey: ["dev", "config"],
    queryFn: () => api.get<DevConfig>("/dev/config"),
    staleTime: Infinity,
    enabled: !isEmbed,
  })

  const { data: embedMe } = useQuery({
    queryKey: ["embed", "me", embedParams?.courseId],
    queryFn: () => {
      const { courseId, token } = embedParams!
      return fetch(`/api/embed/course/${courseId}/me?token=${encodeURIComponent(token!)}`)
        .then((r) => r.json() as Promise<EmbedMe>)
    },
    enabled: isEmbed && !!embedParams?.token,
    staleTime: Infinity,
  })

  return (
    <div className={`${isEmbed ? "h-dvh" : "min-h-screen"} bg-background text-foreground flex flex-col`}>
      <header className="border-b px-4 sm:px-6 py-4">
        <div className="flex flex-wrap items-center justify-between gap-x-3 gap-y-2 max-w-7xl mx-auto min-w-0">
          {isEmbed ? (
            <a href="/" target="_blank" rel="noopener noreferrer" className="text-xl font-bold tracking-tight hover:opacity-80 flex items-center gap-1.5">
              Minerva <ExternalLink className="w-4 h-4" />
            </a>
          ) : (
            <Link to="/" className="text-xl font-bold tracking-tight hover:opacity-80">
              Minerva
            </Link>
          )}
          <nav className="flex flex-wrap items-center gap-x-4 gap-y-2 text-sm min-w-0">
            {!isEmbed && user && (user.role === "teacher" || user.role === "admin") && (
              <Link
                to="/teacher"
                className="text-muted-foreground hover:text-foreground"
              >
                Dashboard
              </Link>
            )}
            {!isEmbed && user && user.role === "admin" && (
              <Link
                to="/admin"
                className="text-muted-foreground hover:text-foreground"
              >
                Admin
              </Link>
            )}
            {!isEmbed && devConfig?.dev_mode && devConfig.users ? (
              <DevUserSwitcher users={devConfig.users} />
            ) : !isEmbed && user ? (
              <span className="text-muted-foreground">
                {user.display_name || user.eppn}
              </span>
            ) : null}
            {isEmbed && embedMe && (
              <span className="text-muted-foreground">
                {embedMe.eppn}{embedMe.lti_client_id && ` via LTI (${embedMe.lti_client_id})`}
              </span>
            )}
          </nav>
        </div>
      </header>
      <main className={`${isEmbed ? "flex-1 min-h-0" : "max-w-7xl mx-auto px-4 sm:px-6 py-8 flex-1 w-full min-w-0"}`}>
        <Outlet />
      </main>
      <footer className="border-t px-4 sm:px-6 py-4 mt-auto">
        <div className="flex flex-wrap items-center justify-between gap-2 max-w-7xl mx-auto text-xs text-muted-foreground">
          <span>
            <a href="https://github.com/Edwinexd/minerva" target="_blank" rel="noopener noreferrer" className="hover:text-foreground underline">Minerva</a>
            {" "}is licensed under{" "}
            <a href="https://github.com/Edwinexd/minerva?tab=AGPL-3.0-1-ov-file" target="_blank" rel="noopener noreferrer" className="hover:text-foreground underline">AGPL-3.0</a>
          </span>
          <a href="mailto:lambda@dsv.su.se" className="hover:text-foreground underline">lambda@dsv.su.se</a>
        </div>
      </footer>
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
        className="border rounded px-2 py-1 text-sm bg-background max-w-[12rem] min-w-0"
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
