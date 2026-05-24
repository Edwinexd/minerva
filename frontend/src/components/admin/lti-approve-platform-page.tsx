import { useMemo, useState } from "react"
import { useTranslation } from "react-i18next"
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query"
import { Link, useNavigate } from "@tanstack/react-router"
import { Route as ApproveRoute } from "@/routes/admin/lti.approve.$platformId"
import { adminLtiPlatformsQuery } from "@/lib/queries"
import { api } from "@/lib/api"
import { useApiErrorMessage } from "@/lib/use-api-error"
import { useDocumentTitle } from "@/lib/use-document-title"
import { Badge } from "@/components/ui/badge"
import { Button, buttonVariants } from "@/components/ui/button"
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"
import { Skeleton } from "@/components/ui/skeleton"

/**
 * Focused approve flow for a single pending LTI platform. Deep-linked
 * from the dynreg iframe's "Open Minerva to approve" button, so the
 * integrator can confirm one specific platform without hunting through
 * the full /admin/lti list. Shows enough verification context (issuer,
 * client id, dynreg-suggested scope) for a confident yes/no, plus an
 * editable scope field. On approve, redirects to the full platforms
 * list so subsequent admin work continues there.
 */
export function LtiApprovePlatformPage() {
  const { t } = useTranslation("admin")
  const { t: tCommon } = useTranslation("common")
  useDocumentTitle(tCommon("pageTitles.ltiApprove"))
  const formatError = useApiErrorMessage()

  const { platformId } = ApproveRoute.useParams()
  const navigate = useNavigate()
  const queryClient = useQueryClient()

  const { data: platforms, isLoading } = useQuery(adminLtiPlatformsQuery)
  const platform = useMemo(
    () => platforms?.find((p) => p.id === platformId) ?? null,
    [platforms, platformId],
  )

  const [scopeInput, setScopeInput] = useState("")
  const [scopeSeeded, setScopeSeeded] = useState(false)
  if (platform && !scopeSeeded) {
    setScopeInput(platform.allowed_eppn_domains.join(", "))
    setScopeSeeded(true)
  }

  const approveMutation = useMutation({
    mutationFn: (domains: string[]) =>
      api.post(`/admin/lti/platforms/${platformId}/approve`, {
        allowed_eppn_domains: domains,
      }),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["admin", "lti", "platforms"] })
      navigate({ to: "/admin/lti" })
    },
  })

  const submit = (e: React.FormEvent) => {
    e.preventDefault()
    const domains = scopeInput
      .split(",")
      .map((s) => s.trim())
      .filter((s) => s.length > 0)
    if (domains.length === 0) {
      if (!window.confirm(t("ltiPlatforms.approveEmptyConfirm"))) {
        return
      }
    }
    approveMutation.mutate(domains)
  }

  if (isLoading) {
    return (
      <Card>
        <CardContent className="space-y-2 pt-6">
          <Skeleton className="h-8 w-2/3" />
          <Skeleton className="h-8 w-full" />
          <Skeleton className="h-24 w-full" />
        </CardContent>
      </Card>
    )
  }

  if (!platform) {
    return (
      <Card>
        <CardHeader>
          <CardTitle>{t("ltiApprovePlatform.notFoundTitle")}</CardTitle>
          <CardDescription>
            {t("ltiApprovePlatform.notFoundBody")}
          </CardDescription>
        </CardHeader>
          <CardContent>
          <Link to="/admin/lti" className={buttonVariants()}>
            {t("ltiApprovePlatform.backToList")}
          </Link>
        </CardContent>
      </Card>
    )
  }

  const alreadyActive = platform.activated_at !== null

  return (
    <Card>
      <CardHeader>
        <CardTitle>
          {alreadyActive
            ? t("ltiApprovePlatform.alreadyActiveTitle")
            : t("ltiApprovePlatform.title")}
        </CardTitle>
        <CardDescription>
          {alreadyActive
            ? t("ltiApprovePlatform.alreadyActiveBody")
            : t("ltiApprovePlatform.description")}
        </CardDescription>
      </CardHeader>
      <CardContent className="space-y-4">
        <dl className="grid grid-cols-[max-content_1fr] gap-x-4 gap-y-2 text-sm">
          <dt className="text-muted-foreground">
            {t("ltiApprovePlatform.nameLabel")}
          </dt>
          <dd className="font-medium">{platform.name}</dd>
          <dt className="text-muted-foreground">
            {t("ltiApprovePlatform.issuerLabel")}
          </dt>
          <dd className="font-mono break-all">{platform.issuer}</dd>
          <dt className="text-muted-foreground">
            {t("ltiApprovePlatform.clientIdLabel")}
          </dt>
          <dd>
            <Badge variant="secondary" className="font-mono">
              {platform.client_id}
            </Badge>
          </dd>
        </dl>

        {alreadyActive ? (
          <Link to="/admin/lti" className={buttonVariants()}>
            {t("ltiApprovePlatform.backToList")}
          </Link>
        ) : (
          <form onSubmit={submit} className="space-y-3">
            <div className="space-y-1">
              <Label
                htmlFor="lti-approve-scope"
                className="text-sm font-medium"
              >
                {t("ltiPlatforms.approveScopeLabel")}
              </Label>
              <Input
                id="lti-approve-scope"
                value={scopeInput}
                onChange={(e) => setScopeInput(e.target.value)}
                placeholder="dsv.su.se, su.se"
                className="font-mono"
              />
              <p className="text-xs text-muted-foreground">
                {t("ltiPlatforms.approveScopeHint")}
              </p>
            </div>

            {approveMutation.isError && (
              <p className="text-sm text-destructive">
                {formatError(approveMutation.error)}
              </p>
            )}

            <div className="flex flex-wrap gap-2">
              <Button type="submit" disabled={approveMutation.isPending}>
                {t("ltiPlatforms.approveAndActivate")}
              </Button>
              <Link
                to="/admin/lti"
                className={buttonVariants({ variant: "outline" })}
              >
                {tCommon("actions.cancel")}
              </Link>
            </div>
          </form>
        )}
      </CardContent>
    </Card>
  )
}
