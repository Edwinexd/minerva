import { createFileRoute } from "@tanstack/react-router"
import { RelativeTime } from "@/components/relative-time"
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query"
import { useTranslation } from "react-i18next"
import { useState } from "react"
import { externalAuthInvitesQuery } from "@/lib/queries"
import { api } from "@/lib/api"
import { useApiErrorMessage } from "@/lib/use-api-error"
import type { ExternalAuthInvite, ExternalAuthInviteCreated } from "@/lib/types"
import { Button } from "@/components/ui/button"
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card"
import { Badge } from "@/components/ui/badge"
import { Skeleton } from "@/components/ui/skeleton"

export const Route = createFileRoute("/admin/external-invites")({
  component: ExternalInvitesPanel,
})

function statusOf(invite: ExternalAuthInvite): "active" | "revoked" | "expired" {
  if (invite.revoked_at) return "revoked"
  if (new Date(invite.expires_at).getTime() < Date.now()) return "expired"
  return "active"
}

function ExternalInvitesPanel() {
  const { t } = useTranslation("admin")
  const { data: invites, isLoading } = useQuery(externalAuthInvitesQuery)
  const [lastCreated, setLastCreated] = useState<ExternalAuthInviteCreated | null>(null)

  return (
    <div className="space-y-6">
      <Card>
        <CardHeader>
          <CardTitle>{t("externalInvites.createTitle")}</CardTitle>
          <CardDescription>{t("externalInvites.createDescription")}</CardDescription>
        </CardHeader>
        <CardContent>
          <CreateInviteForm onCreated={setLastCreated} />
          {lastCreated && <CreatedInviteCallout invite={lastCreated} onDismiss={() => setLastCreated(null)} />}
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle>{t("externalInvites.existingTitle", { total: invites?.length ?? 0 })}</CardTitle>
          <CardDescription>{t("externalInvites.existingDescription")}</CardDescription>
        </CardHeader>
        <CardContent>
          {isLoading ? (
            <div className="space-y-2">
              {Array.from({ length: 3 }).map((_, i) => (
                <Skeleton key={i} className="h-12 w-full" />
              ))}
            </div>
          ) : !invites || invites.length === 0 ? (
            <p className="text-sm text-muted-foreground">{t("externalInvites.empty")}</p>
          ) : (
            <div className="overflow-x-auto">
              <table className="w-full text-sm">
                <thead>
                  <tr className="border-b text-left">
                    <th className="py-2 pr-4 font-medium">{t("externalInvites.columns.eppn")}</th>
                    <th className="py-2 pr-4 font-medium">{t("externalInvites.columns.displayName")}</th>
                    <th className="py-2 pr-4 font-medium">{t("externalInvites.columns.created")}</th>
                    <th className="py-2 pr-4 font-medium">{t("externalInvites.columns.expires")}</th>
                    <th className="py-2 pr-4 font-medium">{t("externalInvites.columns.status")}</th>
                    <th className="py-2 font-medium">{t("externalInvites.columns.actions")}</th>
                  </tr>
                </thead>
                <tbody>
                  {invites.map((inv) => (
                    <InviteRow key={inv.id} invite={inv} />
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

function CreateInviteForm({ onCreated }: { onCreated: (inv: ExternalAuthInviteCreated) => void }) {
  const { t } = useTranslation("admin")
  const formatError = useApiErrorMessage()
  const queryClient = useQueryClient()
  const [eppn, setEppn] = useState("")
  const [displayName, setDisplayName] = useState("")
  const [days, setDays] = useState(7)

  const mutation = useMutation({
    mutationFn: () =>
      api.post<ExternalAuthInviteCreated>("/admin/external-invites", {
        eppn: eppn.trim(),
        display_name: displayName.trim() || null,
        days,
      }),
    onSuccess: (created) => {
      onCreated(created)
      setEppn("")
      setDisplayName("")
      setDays(7)
      queryClient.invalidateQueries({ queryKey: ["admin", "external-invites"] })
    },
  })

  const submit = (e: React.FormEvent) => {
    e.preventDefault()
    if (!eppn.trim()) return
    mutation.mutate()
  }

  return (
    <form onSubmit={submit} className="grid gap-3 sm:grid-cols-[2fr,2fr,1fr,auto] sm:items-end">
      <label className="block">
        <span className="mb-1 block text-xs font-medium">{t("externalInvites.form.identifier")}</span>
        <input
          required
          className="w-full rounded border bg-background px-3 py-1.5 text-sm"
          placeholder={t("externalInvites.form.identifierPlaceholder")}
          value={eppn}
          onChange={(e) => setEppn(e.target.value)}
        />
      </label>
      <label className="block">
        <span className="mb-1 block text-xs font-medium">{t("externalInvites.form.displayName")}</span>
        <input
          className="w-full rounded border bg-background px-3 py-1.5 text-sm"
          placeholder={t("externalInvites.form.displayNamePlaceholder")}
          value={displayName}
          onChange={(e) => setDisplayName(e.target.value)}
        />
      </label>
      <label className="block">
        <span className="mb-1 block text-xs font-medium">{t("externalInvites.form.days")}</span>
        <input
          type="number"
          min={1}
          max={60}
          className="w-full rounded border bg-background px-3 py-1.5 text-sm"
          value={days}
          onChange={(e) => setDays(Math.max(1, Math.min(60, Number(e.target.value) || 1)))}
        />
      </label>
      <Button type="submit" disabled={mutation.isPending || !eppn.trim()}>
        {mutation.isPending ? t("externalInvites.form.creating") : t("externalInvites.form.create")}
      </Button>
      {mutation.isError && (
        <p className="sm:col-span-4 text-xs text-destructive">{formatError(mutation.error)}</p>
      )}
    </form>
  )
}

function CreatedInviteCallout({
  invite,
  onDismiss,
}: {
  invite: ExternalAuthInviteCreated
  onDismiss: () => void
}) {
  const { t } = useTranslation("admin")
  const [copied, setCopied] = useState(false)

  const copy = async () => {
    await navigator.clipboard.writeText(invite.url)
    setCopied(true)
    setTimeout(() => setCopied(false), 1500)
  }

  return (
    <div className="mt-4 rounded-md border border-amber-300 bg-amber-50 p-3 text-sm dark:border-amber-700 dark:bg-amber-950/40">
      <div className="mb-2 flex items-center justify-between">
        <strong>{t("externalInvites.callout.title", { eppn: invite.eppn })}</strong>
        <button
          type="button"
          className="text-xs text-muted-foreground hover:underline"
          onClick={onDismiss}
        >
          {t("externalInvites.callout.dismiss")}
        </button>
      </div>
      <p className="mb-2 text-xs text-muted-foreground">
        {t("externalInvites.callout.note")}
        <RelativeTime date={invite.expires_at} />.
      </p>
      <div className="flex gap-2">
        <input
          readOnly
          value={invite.url}
          className="flex-1 rounded border bg-background px-2 py-1 font-mono text-xs"
          onFocus={(e) => e.currentTarget.select()}
        />
        <Button type="button" size="sm" variant="outline" onClick={copy}>
          {copied ? t("externalInvites.callout.copied") : t("externalInvites.callout.copy")}
        </Button>
      </div>
    </div>
  )
}

function InviteRow({ invite }: { invite: ExternalAuthInvite }) {
  const { t } = useTranslation("admin")
  const queryClient = useQueryClient()
  const status = statusOf(invite)

  const revokeMutation = useMutation({
    mutationFn: () => api.delete(`/admin/external-invites/${invite.id}`),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["admin", "external-invites"] })
    },
  })

  return (
    <tr className="border-b">
      <td className="py-2 pr-4 font-mono text-xs">{invite.eppn}</td>
      <td className="py-2 pr-4">{invite.display_name ?? "-"}</td>
      <td className="py-2 pr-4 text-xs"><RelativeTime date={invite.created_at} /></td>
      <td className="py-2 pr-4 text-xs"><RelativeTime date={invite.expires_at} /></td>
      <td className="py-2 pr-4">
        {status === "active" && <Badge variant="secondary">{t("externalInvites.status.active")}</Badge>}
        {status === "expired" && <Badge variant="outline">{t("externalInvites.status.expired")}</Badge>}
        {status === "revoked" && <Badge variant="destructive">{t("externalInvites.status.revoked")}</Badge>}
      </td>
      <td className="py-2">
        {status === "active" && (
          <Button
            variant="destructive"
            size="sm"
            className="h-7 text-xs"
            onClick={() => revokeMutation.mutate()}
            disabled={revokeMutation.isPending}
          >
            {t("externalInvites.revoke")}
          </Button>
        )}
      </td>
    </tr>
  )
}
