import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query"
import { useTranslation } from "react-i18next"
import { api } from "@/lib/api"
import { mcpConnectionsQuery } from "@/lib/queries"
import { Button } from "@/components/ui/button"
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card"
import { ErrorText } from "@/components/ui/error-text"
import { Skeleton } from "@/components/ui/skeleton"

/**
 * Connect an AI assistant (any MCP client) to Minerva over MCP, and
 * see or revoke the assistants already connected. Read-only access to
 * what the user can see as staff; students never get here.
 */
export function AssistantsPage() {
  const { t, i18n } = useTranslation("teacher")
  const queryClient = useQueryClient()
  const { data, isLoading, error } = useQuery(mcpConnectionsQuery)
  const revoke = useMutation({
    mutationFn: (id: string) => api.delete(`/oauth/connections/${id}`),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: mcpConnectionsQuery.queryKey }),
  })
  const format = new Intl.DateTimeFormat(i18n.language, { dateStyle: "medium" })

  if (isLoading) return <Skeleton className="h-40 w-full" />
  if (error || !data) return <ErrorText error={error} />

  return (
    <div className="grid gap-4 lg:grid-cols-[3fr_2fr]">
      <Card>
        <CardHeader>
          <CardTitle>{t("assistants.connectTitle")}</CardTitle>
          <CardDescription>{t("assistants.connectBody")}</CardDescription>
        </CardHeader>
        <CardContent className="space-y-4">
          {data.allowed ? (
            <div className="space-y-1">
              <p className="text-sm font-medium" id="mcp-url-label">
                {t("assistants.urlLabel")}
              </p>
              <div className="flex gap-2">
                <code
                  aria-labelledby="mcp-url-label"
                  className="flex-1 rounded-md bg-muted px-3 py-2 text-sm break-all"
                >
                  {data.mcp_url}
                </code>
                <Button
                  type="button"
                  variant="outline"
                  onClick={() => void navigator.clipboard?.writeText(data.mcp_url)}
                >
                  {t("assistants.copy")}
                </Button>
              </div>
            </div>
          ) : (
            <p className="text-sm text-muted-foreground">{t("assistants.notAllowed")}</p>
          )}
          <div className="space-y-1 text-sm">
            <p className="font-medium">{t("assistants.toolsTitle")}</p>
            <ul className="list-disc pl-5 text-muted-foreground space-y-0.5">
              <li>{t("assistants.tools.listWorkspaces")}</li>
              <li>{t("assistants.tools.search")}</li>
              <li>{t("assistants.tools.documents")}</li>
              <li>{t("assistants.tools.plans")}</li>
            </ul>
          </div>
          <p className="text-xs text-muted-foreground">{t("assistants.privacy")}</p>
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle>{t("assistants.connectedTitle")}</CardTitle>
        </CardHeader>
        <CardContent>
          {data.connections.length === 0 ? (
            <p className="text-sm text-muted-foreground">{t("assistants.none")}</p>
          ) : (
            <ul className="divide-y">
              {data.connections.map((c) => (
                <li key={c.id} className="flex items-start justify-between gap-3 py-3">
                  <div className="min-w-0">
                    <p className="font-medium truncate" title={c.client_id}>
                      {c.client_name}
                    </p>
                    <p className="text-xs text-muted-foreground">
                      {t("assistants.connectedOn", { date: format.format(new Date(c.connected_at)) })}
                      {c.last_used_at &&
                        ` · ${t("assistants.lastUsed", { date: format.format(new Date(c.last_used_at)) })}`}
                    </p>
                  </div>
                  <Button
                    type="button"
                    variant="outline"
                    size="sm"
                    disabled={revoke.isPending}
                    onClick={() => revoke.mutate(c.id)}
                    aria-label={t("assistants.revokeNamed", { name: c.client_name })}
                  >
                    {t("assistants.revoke")}
                  </Button>
                </li>
              ))}
            </ul>
          )}
          {revoke.isError && <ErrorText error={revoke.error} />}
        </CardContent>
      </Card>
    </div>
  )
}
