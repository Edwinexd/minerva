import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query"
import { useTranslation } from "react-i18next"
import { useState } from "react"
import { adminIntegrationKeysQuery } from "@/lib/queries"
import { api } from "@/lib/api"
import { useApiErrorMessage } from "@/lib/use-api-error"
import type { SiteIntegrationKey, SiteIntegrationKeyCreated } from "@/lib/types"
import { RelativeTime } from "@/components/relative-time"
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
import { Skeleton } from "@/components/ui/skeleton"
import { Badge } from "@/components/ui/badge"

/// Admin page for site-level integration keys used by the Moodle / Canvas
/// plugin. A key here can provision per-course api_keys on behalf of any
/// Moodle user that maps to an eppn with a course they own/teach; it
/// cannot access course data itself. See routes/integration_admin.rs.
export function IntegrationKeysPanel() {
  const { t } = useTranslation("admin")
  const { data: keys, isLoading } = useQuery(adminIntegrationKeysQuery)
  const [lastCreated, setLastCreated] = useState<SiteIntegrationKeyCreated | null>(null)

  return (
    <div className="space-y-6">
      <Card>
        <CardHeader>
          <CardTitle>{t("integrationKeys.createTitle")}</CardTitle>
          <CardDescription>{t("integrationKeys.createDescription")}</CardDescription>
        </CardHeader>
        <CardContent>
          <CreateKeyForm onCreated={setLastCreated} />
          {lastCreated && (
            <CreatedKeyCallout
              created={lastCreated}
              onDismiss={() => setLastCreated(null)}
            />
          )}
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle>
            {t("integrationKeys.listTitle", { total: keys?.length ?? 0 })}
          </CardTitle>
          <CardDescription>{t("integrationKeys.listDescription")}</CardDescription>
        </CardHeader>
        <CardContent>
          {isLoading ? (
            <div className="space-y-2">
              {Array.from({ length: 3 }).map((_, i) => (
                <Skeleton key={i} className="h-12 w-full" />
              ))}
            </div>
          ) : !keys || keys.length === 0 ? (
            <p className="text-sm text-muted-foreground">
              {t("integrationKeys.empty")}
            </p>
          ) : (
            <div className="overflow-x-auto">
              <table className="w-full text-sm">
                <thead>
                  <tr className="border-b text-left">
                    <th className="py-2 pr-4 font-medium">
                      {t("integrationKeys.columns.name")}
                    </th>
                    <th className="py-2 pr-4 font-medium">
                      {t("integrationKeys.columns.prefix")}
                    </th>
                    <th className="py-2 pr-4 font-medium">
                      {t("integrationKeys.columns.scope")}
                    </th>
                    <th className="py-2 pr-4 font-medium">
                      {t("integrationKeys.columns.created")}
                    </th>
                    <th className="py-2 pr-4 font-medium">
                      {t("integrationKeys.columns.lastUsed")}
                    </th>
                    <th className="py-2 font-medium">
                      {t("integrationKeys.columns.actions")}
                    </th>
                  </tr>
                </thead>
                <tbody>
                  {keys.map((k) => (
                    <KeyRow key={k.id} k={k} />
                  ))}
                </tbody>
              </table>
            </div>
          )}
        </CardContent>
      </Card>
    </div>
  )
}

function CreateKeyForm({
  onCreated,
}: {
  onCreated: (k: SiteIntegrationKeyCreated) => void
}) {
  const { t } = useTranslation("admin")
  const formatError = useApiErrorMessage()
  const queryClient = useQueryClient()
  const [name, setName] = useState("")
  // Free-form comma/space/newline separated list; parsed on submit. Admins
  // paste things like `@dsv.su.se, @su.se` or `dsv.su.se su.se` and we
  // sort it out server-side (trim + strip leading @ + lowercase).
  const [domainsRaw, setDomainsRaw] = useState("")

  const mutation = useMutation({
    mutationFn: () =>
      api.post<SiteIntegrationKeyCreated>("/admin/integration-keys", {
        name: name.trim(),
        allowed_eppn_domains: parseDomains(domainsRaw),
      }),
    onSuccess: (created) => {
      onCreated(created)
      setName("")
      setDomainsRaw("")
      queryClient.invalidateQueries({ queryKey: ["admin", "integration-keys"] })
    },
  })

  return (
    <form
      onSubmit={(e) => {
        e.preventDefault()
        if (!name.trim()) return
        mutation.mutate()
      }}
      className="space-y-3"
    >
      <div className="grid gap-3 sm:grid-cols-[3fr,2fr]">
        <div className="space-y-1">
          <Label htmlFor="site-key-name" className="text-xs font-medium">
            {t("integrationKeys.form.name")}
          </Label>
          <Input
            id="site-key-name"
            required
            placeholder={t("integrationKeys.form.namePlaceholder")}
            value={name}
            onChange={(e) => setName(e.target.value)}
          />
        </div>
        <div className="space-y-1">
          <Label htmlFor="site-key-domains" className="text-xs font-medium">
            {t("integrationKeys.form.domains")}
          </Label>
          <Input
            id="site-key-domains"
            placeholder={t("integrationKeys.form.domainsPlaceholder")}
            value={domainsRaw}
            onChange={(e) => setDomainsRaw(e.target.value)}
          />
          <p className="text-xs text-muted-foreground">
            {t("integrationKeys.form.domainsHelp")}
          </p>
        </div>
      </div>
      <Button type="submit" disabled={mutation.isPending || !name.trim()}>
        {mutation.isPending ? t("integrationKeys.form.creating") : t("integrationKeys.form.create")}
      </Button>
      {mutation.isError && (
        <p className="text-xs text-destructive">{formatError(mutation.error)}</p>
      )}
    </form>
  )
}

/// Parse the free-form domain entry box. Splits on commas / whitespace /
/// newlines so admins can paste in any reasonable style. Server does
/// additional normalisation + validation, so we just need to get a
/// non-empty list over the wire.
function parseDomains(raw: string): string[] {
  return raw
    .split(/[\s,]+/)
    .map((d) => d.trim())
    .filter((d) => d.length > 0)
}

function CreatedKeyCallout({
  created,
  onDismiss,
}: {
  created: SiteIntegrationKeyCreated
  onDismiss: () => void
}) {
  const { t } = useTranslation("admin")
  const [copied, setCopied] = useState(false)
  const copy = async () => {
    await navigator.clipboard.writeText(created.key)
    setCopied(true)
    setTimeout(() => setCopied(false), 1500)
  }

  return (
    <div className="mt-4 rounded-md border border-amber-300 bg-amber-50 p-3 text-sm dark:border-amber-700 dark:bg-amber-950/40">
      <div className="mb-2 flex items-center justify-between">
        <strong>{t("integrationKeys.callout.title", { name: created.name })}</strong>
        <button
          type="button"
          className="text-xs text-muted-foreground hover:underline"
          onClick={onDismiss}
        >
          {t("integrationKeys.callout.dismiss")}
        </button>
      </div>
      <p className="mb-2 text-xs text-muted-foreground">
        {t("integrationKeys.callout.note")}
      </p>
      <div className="flex gap-2">
        <input
          readOnly
          value={created.key}
          className="flex-1 rounded border bg-background px-2 py-1 font-mono text-xs"
          onFocus={(e) => e.currentTarget.select()}
        />
        <Button type="button" size="sm" variant="outline" onClick={copy}>
          {copied ? t("integrationKeys.callout.copied") : t("integrationKeys.callout.copy")}
        </Button>
      </div>
    </div>
  )
}

function KeyRow({ k }: { k: SiteIntegrationKey }) {
  const { t } = useTranslation("admin")
  const queryClient = useQueryClient()
  const deleteMutation = useMutation({
    mutationFn: () => api.delete(`/admin/integration-keys/${k.id}`),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["admin", "integration-keys"] })
    },
  })

  return (
    <tr className="border-b">
      <td className="py-2 pr-4">{k.name}</td>
      <td className="py-2 pr-4 font-mono text-xs">
        <Badge variant="secondary">{k.key_prefix}</Badge>
      </td>
      <td className="py-2 pr-4 text-xs">
        {k.allowed_eppn_domains.length === 0 ? (
          <span className="text-amber-600 dark:text-amber-400">
            {t("integrationKeys.scope.any")}
          </span>
        ) : (
          <div className="flex flex-wrap gap-1">
            {k.allowed_eppn_domains.map((d) => (
              <Badge key={d} variant="outline" className="font-mono">
                @{d}
              </Badge>
            ))}
          </div>
        )}
      </td>
      <td className="py-2 pr-4 text-xs">
        <RelativeTime date={k.created_at} />
      </td>
      <td className="py-2 pr-4 text-xs">
        {k.last_used_at ? (
          <RelativeTime date={k.last_used_at} />
        ) : (
          <span className="text-muted-foreground">
            {t("integrationKeys.neverUsed")}
          </span>
        )}
      </td>
      <td className="py-2">
        <Button
          variant="destructive"
          size="sm"
          className="h-7 text-xs"
          onClick={() => deleteMutation.mutate()}
          disabled={deleteMutation.isPending}
        >
          {t("integrationKeys.revoke")}
        </Button>
      </td>
    </tr>
  )
}
