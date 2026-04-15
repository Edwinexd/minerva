import { createFileRoute } from "@tanstack/react-router"
import { RelativeTime } from "@/components/relative-time"
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query"
import { useState } from "react"
import { externalAuthInvitesQuery } from "@/lib/queries"
import { api } from "@/lib/api"
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
  const { data: invites, isLoading } = useQuery(externalAuthInvitesQuery)
  const [lastCreated, setLastCreated] = useState<ExternalAuthInviteCreated | null>(null)

  return (
    <div className="space-y-6">
      <Card>
        <CardHeader>
          <CardTitle>Create External Invite</CardTitle>
          <CardDescription>
            Generate a time-limited login link for someone without a Stockholm University
            account. Links bypass Shibboleth and grant student-level access; promote to
            teacher in User Management after they first log in. The full link is shown only
            once -- if lost, revoke and re-mint.
          </CardDescription>
        </CardHeader>
        <CardContent>
          <CreateInviteForm onCreated={setLastCreated} />
          {lastCreated && <CreatedInviteCallout invite={lastCreated} onDismiss={() => setLastCreated(null)} />}
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle>Existing Invites ({invites?.length ?? 0})</CardTitle>
          <CardDescription>Active, expired, and revoked invites.</CardDescription>
        </CardHeader>
        <CardContent>
          {isLoading ? (
            <div className="space-y-2">
              {Array.from({ length: 3 }).map((_, i) => (
                <Skeleton key={i} className="h-12 w-full" />
              ))}
            </div>
          ) : !invites || invites.length === 0 ? (
            <p className="text-sm text-muted-foreground">No invites yet.</p>
          ) : (
            <div className="overflow-x-auto">
              <table className="w-full text-sm">
                <thead>
                  <tr className="border-b text-left">
                    <th className="py-2 pr-4 font-medium">Eppn</th>
                    <th className="py-2 pr-4 font-medium">Display name</th>
                    <th className="py-2 pr-4 font-medium">Created</th>
                    <th className="py-2 pr-4 font-medium">Expires</th>
                    <th className="py-2 pr-4 font-medium">Status</th>
                    <th className="py-2 font-medium">Actions</th>
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
        <span className="mb-1 block text-xs font-medium">Identifier (e.g. email)</span>
        <input
          required
          className="w-full rounded border bg-background px-3 py-1.5 text-sm"
          placeholder="alice@example.com"
          value={eppn}
          onChange={(e) => setEppn(e.target.value)}
        />
      </label>
      <label className="block">
        <span className="mb-1 block text-xs font-medium">Display name (optional)</span>
        <input
          className="w-full rounded border bg-background px-3 py-1.5 text-sm"
          placeholder="Alice Example"
          value={displayName}
          onChange={(e) => setDisplayName(e.target.value)}
        />
      </label>
      <label className="block">
        <span className="mb-1 block text-xs font-medium">Days</span>
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
        {mutation.isPending ? "Creating..." : "Create invite"}
      </Button>
      {mutation.isError && (
        <p className="sm:col-span-4 text-xs text-destructive">{mutation.error.message}</p>
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
  const [copied, setCopied] = useState(false)

  const copy = async () => {
    await navigator.clipboard.writeText(invite.url)
    setCopied(true)
    setTimeout(() => setCopied(false), 1500)
  }

  return (
    <div className="mt-4 rounded-md border border-amber-300 bg-amber-50 p-3 text-sm dark:border-amber-700 dark:bg-amber-950/40">
      <div className="mb-2 flex items-center justify-between">
        <strong>Invite link for {invite.eppn}</strong>
        <button
          type="button"
          className="text-xs text-muted-foreground hover:underline"
          onClick={onDismiss}
        >
          dismiss
        </button>
      </div>
      <p className="mb-2 text-xs text-muted-foreground">
        Share this link privately. It is shown only once and grants login until{" "}
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
          {copied ? "Copied" : "Copy"}
        </Button>
      </div>
    </div>
  )
}

function InviteRow({ invite }: { invite: ExternalAuthInvite }) {
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
        {status === "active" && <Badge variant="secondary">active</Badge>}
        {status === "expired" && <Badge variant="outline">expired</Badge>}
        {status === "revoked" && <Badge variant="destructive">revoked</Badge>}
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
            Revoke
          </Button>
        )}
      </td>
    </tr>
  )
}
