import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query"
import { useTranslation } from "react-i18next"
import { useState } from "react"
import {
  adminLtiPlatformBindingsQuery,
  adminLtiPlatformNrpsQuery,
  adminLtiPlatformsQuery,
  adminLtiSetupQuery,
} from "@/lib/queries"
import { api } from "@/lib/api"
import { useApiErrorMessage } from "@/lib/use-api-error"
import type { LtiPlatform } from "@/lib/types"
import { Button } from "@/components/ui/button"
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"
import { Badge } from "@/components/ui/badge"
import { Separator } from "@/components/ui/separator"
import { Skeleton } from "@/components/ui/skeleton"
import { RelativeTime } from "@/components/relative-time"

export function LtiPlatformsPanel() {
  const { t } = useTranslation("admin")
  const { t: tCommon } = useTranslation("common")
  const queryClient = useQueryClient()
  const formatError = useApiErrorMessage()
  const { data: setup } = useQuery(adminLtiSetupQuery)
  const { data: platforms, isLoading } = useQuery(adminLtiPlatformsQuery)
  const [showForm, setShowForm] = useState(false)
  const [name, setName] = useState("")
  const [issuer, setIssuer] = useState("")
  const [clientId, setClientId] = useState("")
  const [deploymentId, setDeploymentId] = useState("")
  // Free-form domain input; parsed to a string[] at submit time. Mirrors
  // the site-integration-keys picker so admins don't see two different
  // syntaxes for the same concept.
  const [domainsRaw, setDomainsRaw] = useState("")
  const [copiedField, setCopiedField] = useState<string | null>(null)

  const createMutation = useMutation({
    mutationFn: (data: {
      name: string
      issuer: string
      client_id: string
      deployment_id: string | null
      allowed_eppn_domains: string[]
    }) => api.post<LtiPlatform>("/admin/lti/platforms", data),
    onSuccess: () => {
      setShowForm(false)
      setName("")
      setIssuer("")
      setClientId("")
      setDeploymentId("")
      setDomainsRaw("")
      queryClient.invalidateQueries({ queryKey: ["admin", "lti", "platforms"] })
    },
  })

  const deleteMutation = useMutation({
    mutationFn: (id: string) => api.delete(`/admin/lti/platforms/${id}`),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["admin", "lti", "platforms"] })
    },
  })

  const config = setup?.moodle_tool_config

  function copyToClipboard(text: string, field: string) {
    navigator.clipboard.writeText(text)
    setCopiedField(field)
    setTimeout(() => setCopiedField(null), 2000)
  }

  return (
    <div className="space-y-6">
      <Card>
        <CardHeader>
          <CardTitle>{t("ltiPlatforms.setupTitle")}</CardTitle>
          <CardDescription>{t("ltiPlatforms.setupDescription")}</CardDescription>
        </CardHeader>
        <CardContent className="space-y-4">
          {!setup ? (
            <div className="space-y-2">
              <Skeleton className="h-8 w-full" />
              <Skeleton className="h-8 w-full" />
              <Skeleton className="h-8 w-full" />
            </div>
          ) : (
            <>
              {setup.dynamic_registration_url && (
                <div className="space-y-2 rounded-md border border-primary/30 bg-primary/5 p-4">
                  <div className="flex items-center justify-between gap-2">
                    <h3 className="text-sm font-semibold">
                      {t("ltiPlatforms.dynregHeading")}
                    </h3>
                  </div>
                  <p className="text-sm text-muted-foreground">
                    {t("ltiPlatforms.dynregDescription")}
                  </p>
                  <div className="flex items-center justify-between gap-4">
                    <code className="block flex-1 truncate rounded bg-background px-2 py-1 text-sm">
                      {setup.dynamic_registration_url}
                    </code>
                    <Button
                      variant="default"
                      size="sm"
                      className="shrink-0"
                      onClick={() => copyToClipboard(setup.dynamic_registration_url!, "dynreg")}
                    >
                      {copiedField === "dynreg"
                        ? t("ltiPlatforms.copied")
                        : tCommon("actions.copy")}
                    </Button>
                    <output className="sr-only">
                      {copiedField === "dynreg" ? t("ltiPlatforms.copied") : ""}
                    </output>
                  </div>
                  <p className="text-xs text-muted-foreground">
                    {t("ltiPlatforms.dynregMoodlePath")}
                  </p>
                </div>
              )}

              {config && (
                <details className="group rounded-md border [&_summary::-webkit-details-marker]:hidden">
                  <summary className="flex cursor-pointer items-center gap-2 px-4 py-3 text-sm font-medium select-none">
                    <span aria-hidden className="transition-transform group-open:rotate-90">
                      ▶
                    </span>
                    {t("ltiPlatforms.manualSetupToggle")}
                  </summary>
                  <div className="space-y-3 border-t px-4 py-3">
                    <p className="text-xs text-muted-foreground">
                      {t("ltiPlatforms.manualSetupHint")}
                    </p>
                    {setup.steps.length > 0 && (
                      <div className="space-y-2">
                        <Label className="text-xs font-medium uppercase text-muted-foreground tracking-wide">
                          {t("ltiPlatforms.manualSetupStepsHeading")}
                        </Label>
                        <ol className="list-decimal space-y-1.5 pl-5 text-sm">
                          {setup.steps.map((step, i) => (
                            <li key={i}>{step}</li>
                          ))}
                        </ol>
                      </div>
                    )}
                    <Separator />
                    <Label className="text-xs font-medium uppercase text-muted-foreground tracking-wide">
                      {t("ltiPlatforms.manualSetupValuesHeading")}
                    </Label>
                    {[
                      { label: t("ltiPlatforms.toolUrl"), value: config.tool_url, key: "tool_url" },
                      { label: t("ltiPlatforms.ltiVersion"), value: config.lti_version, key: "lti_version" },
                      { label: t("ltiPlatforms.publicKeyType"), value: config.public_key_type, key: "public_key_type" },
                      { label: t("ltiPlatforms.publicKeysetUrl"), value: config.public_keyset_url, key: "keyset" },
                      { label: t("ltiPlatforms.initiateLoginUrl"), value: config.initiate_login_url, key: "login" },
                      { label: t("ltiPlatforms.redirectionUris"), value: config.redirection_uris, key: "redirect" },
                      { label: t("ltiPlatforms.customParameters"), value: config.custom_parameters, key: "custom" },
                      { label: t("ltiPlatforms.iconUrl"), value: config.icon_url, key: "icon" },
                    ].map(({ label, value, key }) => (
                      <div key={key} className="flex items-center justify-between gap-4">
                        <div className="min-w-0 flex-1">
                          <Label className="text-xs text-muted-foreground">{label}</Label>
                          <code className="block text-sm bg-muted px-2 py-1 rounded truncate">{value}</code>
                        </div>
                        <Button
                          variant="outline"
                          size="sm"
                          className="shrink-0"
                          onClick={() => copyToClipboard(value, key)}
                        >
                          {copiedField === key ? t("ltiPlatforms.copied") : tCommon("actions.copy")}
                        </Button>
                        <output className="sr-only">
                          {copiedField === key ? t("ltiPlatforms.copied") : ""}
                        </output>
                      </div>
                    ))}
                    <Separator />
                    <p className="text-sm text-muted-foreground">
                      {t("ltiPlatforms.siteLevelNote")}
                    </p>
                  </div>
                </details>
              )}
            </>
          )}
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle>{t("ltiPlatforms.listTitle")}</CardTitle>
          <CardDescription>{t("ltiPlatforms.listDescription")}</CardDescription>
        </CardHeader>
        <CardContent className="space-y-4">
          {!showForm && (
            <Button onClick={() => setShowForm(true)}>
              {t("ltiPlatforms.addPlatform")}
            </Button>
          )}

          {showForm && (
            <form
              className="space-y-3 rounded-md border p-4"
              onSubmit={(e) => {
                e.preventDefault()
                createMutation.mutate({
                  name: name.trim(),
                  issuer: issuer.trim(),
                  client_id: clientId.trim(),
                  deployment_id: deploymentId.trim() || null,
                  allowed_eppn_domains: parseDomains(domainsRaw),
                })
              }}
            >
              <p className="text-sm text-muted-foreground">
                {t("ltiPlatforms.copyValuesHint")}
              </p>
              <div className="space-y-2">
                <Label htmlFor="lti-platform-name">{t("ltiPlatforms.nameLabel")}</Label>
                <Input
                  id="lti-platform-name"
                  value={name}
                  onChange={(e) => setName(e.target.value)}
                  placeholder={t("ltiPlatforms.namePlaceholder")}
                />
              </div>
              <div className="space-y-2">
                <Label htmlFor="lti-platform-issuer">{t("ltiPlatforms.issuerLabel")}</Label>
                <Input
                  id="lti-platform-issuer"
                  value={issuer}
                  onChange={(e) => setIssuer(e.target.value)}
                  placeholder={t("ltiPlatforms.issuerPlaceholder")}
                />
              </div>
              <div className="space-y-2">
                <Label htmlFor="lti-platform-client-id">{t("ltiPlatforms.clientIdLabel")}</Label>
                <Input
                  id="lti-platform-client-id"
                  value={clientId}
                  onChange={(e) => setClientId(e.target.value)}
                />
              </div>
              <div className="space-y-2">
                <Label htmlFor="lti-platform-deployment-id">
                  {t("ltiPlatforms.deploymentIdLabel")}
                </Label>
                <Input
                  id="lti-platform-deployment-id"
                  value={deploymentId}
                  onChange={(e) => setDeploymentId(e.target.value)}
                  placeholder={t("ltiPlatforms.deploymentIdPlaceholder")}
                />
              </div>
              <div className="space-y-2">
                <Label htmlFor="lti-platform-domains">
                  {t("ltiPlatforms.domainsLabel")}
                </Label>
                <Input
                  id="lti-platform-domains"
                  value={domainsRaw}
                  onChange={(e) => setDomainsRaw(e.target.value)}
                  placeholder={t("ltiPlatforms.domainsPlaceholder")}
                />
                <p className="text-xs text-muted-foreground">
                  {t("ltiPlatforms.domainsHelp")}
                </p>
              </div>

              {createMutation.isError && (
                <p className="text-sm text-destructive">{formatError(createMutation.error)}</p>
              )}

              <div className="flex gap-2">
                <Button
                  type="submit"
                  disabled={
                    createMutation.isPending ||
                    !issuer.trim() ||
                    !clientId.trim() ||
                    !name.trim()
                  }
                >
                  {createMutation.isPending
                    ? t("ltiPlatforms.saving")
                    : t("ltiPlatforms.savePlatform")}
                </Button>
                <Button type="button" variant="outline" onClick={() => setShowForm(false)}>
                  {tCommon("actions.cancel")}
                </Button>
              </div>
            </form>
          )}

          {isLoading && (
            <div className="space-y-2">
              <Skeleton className="h-10 w-full" />
            </div>
          )}

          {platforms && platforms.length === 0 && !showForm && (
            <p className="text-sm text-muted-foreground py-4 text-center">
              {t("ltiPlatforms.empty")}
            </p>
          )}

          <div className="space-y-2">
            {platforms?.map((p) => (
              <PlatformRow
                key={p.id}
                platform={p}
                onDelete={() => deleteMutation.mutate(p.id)}
                deleting={deleteMutation.isPending}
              />
            ))}
          </div>
        </CardContent>
      </Card>
    </div>
  )
}

function PlatformRow({
  platform,
  onDelete,
  deleting,
}: {
  platform: LtiPlatform
  onDelete: () => void
  deleting: boolean
}) {
  const { t } = useTranslation("admin")
  const [open, setOpen] = useState(false)
  // Bindings fetched lazily so the list view stays cheap when there are many
  // platforms; admins typically only inspect one at a time.
  const { data: bindings, isLoading } = useQuery({
    ...adminLtiPlatformBindingsQuery(platform.id),
    enabled: open,
  })
  const { data: nrps } = useQuery({
    ...adminLtiPlatformNrpsQuery(platform.id),
    enabled: open,
  })

  return (
    <div className="rounded-md border">
      <div className="flex items-center justify-between gap-4 p-3">
        <div className="min-w-0 flex-1 space-y-1">
          <div className="flex items-center gap-2">
            <span className="font-medium text-sm">{platform.name}</span>
            <Badge variant="secondary">{platform.client_id}</Badge>
          </div>
          <div className="text-xs text-muted-foreground truncate">{platform.issuer}</div>
          <div className="flex flex-wrap items-center gap-1 text-xs">
            <span className="text-muted-foreground">
              {t("ltiPlatforms.scopeLabel")}:
            </span>
            {platform.allowed_eppn_domains.length === 0 ? (
              <span className="text-amber-600 dark:text-amber-400">
                {t("ltiPlatforms.scopeAny")}
              </span>
            ) : (
              platform.allowed_eppn_domains.map((d) => (
                <Badge key={d} variant="outline" className="font-mono">
                  @{d}
                </Badge>
              ))
            )}
          </div>
        </div>
        <div className="flex shrink-0 gap-2">
          <Button variant="outline" size="sm" onClick={() => setOpen((o) => !o)}>
            {open ? t("ltiPlatforms.hideBindings") : t("ltiPlatforms.showBindings")}
          </Button>
          <Button variant="destructive" size="sm" onClick={onDelete} disabled={deleting}>
            {t("ltiPlatforms.remove")}
          </Button>
        </div>
      </div>

      {open && (
        <div className="border-t p-3">
          <p className="mb-2 text-xs text-muted-foreground">
            {t("ltiPlatforms.bindingsHint")}
          </p>
          {isLoading && <Skeleton className="h-8 w-full" />}
          {bindings && bindings.length === 0 && (
            <p className="text-sm text-muted-foreground">{t("ltiPlatforms.noBindings")}</p>
          )}
          {bindings && bindings.length > 0 && (
            <table className="w-full text-sm">
              <thead>
                <tr className="border-b text-left">
                  <th className="py-1 pr-3 font-medium">{t("ltiPlatforms.bindingColumns.context")}</th>
                  <th className="py-1 pr-3 font-medium">{t("ltiPlatforms.bindingColumns.course")}</th>
                  <th className="py-1 font-medium">{t("ltiPlatforms.bindingColumns.created")}</th>
                </tr>
              </thead>
              <tbody>
                {bindings.map((b) => (
                  <tr key={b.id} className="border-b last:border-0">
                    <td className="py-1 pr-3 font-mono text-xs">
                      {b.context_title || b.context_label || b.context_id}
                    </td>
                    <td className="py-1 pr-3">{b.course_name ?? b.course_id}</td>
                    <td className="py-1 text-xs">
                      <RelativeTime date={b.created_at} />
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}

          {nrps && nrps.length > 0 && (
            <div className="mt-4">
              <p className="mb-2 text-xs text-muted-foreground">
                {t("ltiPlatforms.nrpsHint")}
              </p>
              <table className="w-full text-sm">
                <thead>
                  <tr className="border-b text-left">
                    <th className="py-1 pr-3 font-medium">{t("ltiPlatforms.nrpsColumns.context")}</th>
                    <th className="py-1 pr-3 font-medium">{t("ltiPlatforms.nrpsColumns.status")}</th>
                    <th className="py-1 pr-3 font-medium">{t("ltiPlatforms.nrpsColumns.changes")}</th>
                    <th className="py-1 font-medium">{t("ltiPlatforms.nrpsColumns.lastSync")}</th>
                  </tr>
                </thead>
                <tbody>
                  {nrps.map((ctx) => (
                    <tr key={ctx.id} className="border-b last:border-0 align-top">
                      <td className="py-1 pr-3 font-mono text-xs">{ctx.context_id || "-"}</td>
                      <td className="py-1 pr-3">
                        <div className="flex flex-wrap items-center gap-1">
                          {ctx.last_sync_status === "error" ? (
                            <Badge variant="destructive">{t("ltiPlatforms.nrpsStatusError")}</Badge>
                          ) : ctx.last_sync_status === "ok" ? (
                            <Badge variant="secondary">{t("ltiPlatforms.nrpsStatusOk")}</Badge>
                          ) : (
                            <Badge variant="outline">{t("ltiPlatforms.nrpsStatusPending")}</Badge>
                          )}
                          {ctx.last_sync_warning && (
                            <Badge
                              variant="outline"
                              className="border-amber-500 text-amber-700 dark:text-amber-400"
                            >
                              {t("ltiPlatforms.nrpsStatusWarning")}
                            </Badge>
                          )}
                        </div>
                        {ctx.last_sync_status === "error" && ctx.last_sync_error && (
                          <div className="mt-1 text-xs text-destructive break-all">
                            {ctx.last_sync_error}
                          </div>
                        )}
                        {ctx.last_sync_warning && (
                          <div className="mt-1 text-xs text-amber-700 dark:text-amber-400 break-words">
                            {ctx.last_sync_warning}
                          </div>
                        )}
                      </td>
                      <td className="py-1 pr-3 text-xs">
                        {ctx.last_sync_status === "ok"
                          ? t("ltiPlatforms.nrpsCounts", {
                              added: ctx.last_sync_added ?? 0,
                              removed: ctx.last_sync_removed ?? 0,
                            })
                          : "-"}
                      </td>
                      <td className="py-1 text-xs">
                        {ctx.last_sync_at ? (
                          <RelativeTime date={ctx.last_sync_at} />
                        ) : (
                          t("ltiPlatforms.nrpsNeverSynced")
                        )}
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          )}
        </div>
      )}
    </div>
  )
}

/// Split free-form input (comma / whitespace / newline separated) into a
/// clean string[]. Server normalises further (strips `@`, lowercases, etc).
function parseDomains(raw: string): string[] {
  return raw
    .split(/[\s,]+/)
    .map((d) => d.trim())
    .filter((d) => d.length > 0)
}
