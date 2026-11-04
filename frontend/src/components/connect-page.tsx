import { useMutation, useQuery } from "@tanstack/react-query"
import { useTranslation } from "react-i18next"
import { api } from "@/lib/api"
import { useDocumentTitle } from "@/lib/use-document-title"
import { Button } from "@/components/ui/button"
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card"
import { ErrorText } from "@/components/ui/error-text"
import { Skeleton } from "@/components/ui/skeleton"

interface PendingConnection {
  id: string
  client_name: string
  client_id: string
  redirect_host: string
  redirect_is_loopback: boolean
  scope: string
}

/**
 * OAuth consent for connecting an AI assistant over MCP. The backend's
 * `/api/oauth/authorize` parks the request and sends the browser here;
 * approving or denying returns the browser to the assistant.
 */
export function ConnectPage({ requestId }: { requestId: string }) {
  const { t } = useTranslation("common")
  useDocumentTitle(t("connect.title"))
  const { data, isLoading, error } = useQuery({
    queryKey: ["oauth", "requests", requestId],
    queryFn: () => api.get<PendingConnection>(`/oauth/requests/${requestId}`),
    retry: false,
  })
  const decide = useMutation({
    mutationFn: (choice: "approve" | "deny") =>
      api.post<{ redirect_to: string }>(`/oauth/requests/${requestId}/${choice}`, {}),
    onSuccess: ({ redirect_to }) => {
      window.location.assign(redirect_to)
    },
  })

  return (
    <div className="mx-auto max-w-lg">
      {isLoading && <Skeleton className="h-56 w-full" />}
      {error && (
        <Card>
          <CardHeader>
            <CardTitle>{t("connect.title")}</CardTitle>
            <CardDescription>{t("connect.expired")}</CardDescription>
          </CardHeader>
        </Card>
      )}
      {data && (
        <Card>
          <CardHeader>
            <CardTitle>{t("connect.heading", { name: data.client_name })}</CardTitle>
            <CardDescription>{t("connect.body")}</CardDescription>
          </CardHeader>
          <CardContent className="space-y-4">
            <ul className="list-disc pl-5 text-sm space-y-1">
              <li>{t("connect.canRead")}</li>
              <li>{t("connect.cannotWrite")}</li>
              <li>{t("connect.revokeHint")}</li>
            </ul>
            <p className="text-sm">
              {t("connect.returnsTo")} <strong>{data.redirect_host}</strong>
            </p>
            {data.redirect_is_loopback && (
              <p className="text-sm text-destructive">{t("connect.loopbackWarning")}</p>
            )}
            <p className="text-xs text-muted-foreground break-all">
              {t("connect.clientId")} {data.client_id}
            </p>
            <div className="flex gap-2">
              <Button disabled={decide.isPending} onClick={() => decide.mutate("approve")}>
                {t("connect.approve")}
              </Button>
              <Button
                variant="outline"
                disabled={decide.isPending}
                onClick={() => decide.mutate("deny")}
              >
                {t("connect.deny")}
              </Button>
            </div>
            {decide.isError && <ErrorText error={decide.error} />}
          </CardContent>
        </Card>
      )}
    </div>
  )
}
