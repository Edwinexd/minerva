import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query"
import { useTranslation } from "react-i18next"
import { Menu } from "@base-ui/react/menu"
import { MoreHorizontalIcon } from "lucide-react"
import { adminUsersQuery } from "@/lib/queries"
import { api } from "@/lib/api"
import { useApiErrorMessage } from "@/lib/use-api-error"
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
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select"
import { useState } from "react"
import type { AdminUser } from "@/lib/types"

export function UserManagementPanel() {
  const { t } = useTranslation("admin")
  const { data: users, isLoading } = useQuery(adminUsersQuery)
  const [filter, setFilter] = useState("")

  if (isLoading) {
    return (
      <div className="space-y-2">
        {Array.from({ length: 5 }).map((_, i) => (
          <Skeleton key={i} className="h-14 w-full" />
        ))}
      </div>
    )
  }

  if (!users) return null

  const filtered = filter
    ? users.filter(
        (u) =>
          u.eppn.toLowerCase().includes(filter.toLowerCase()) ||
          (u.display_name ?? "").toLowerCase().includes(filter.toLowerCase()),
      )
    : users

  return (
    <Card>
      <CardHeader>
        <CardTitle>{t("users.title", { total: users.length })}</CardTitle>
        <CardDescription>{t("users.description")}</CardDescription>
        <input
          className="mt-2 w-full max-w-sm rounded border bg-background px-3 py-1.5 text-sm"
          placeholder={t("users.filterPlaceholder")}
          aria-label={t("users.filterPlaceholder")}
          value={filter}
          onChange={(e) => setFilter(e.target.value)}
        />
      </CardHeader>
      <CardContent>
        <div className="overflow-x-auto">
          <table className="w-full text-sm">
            <thead>
              <tr className="border-b text-left">
                <th className="py-2 pr-4 font-medium">{t("users.columns.user")}</th>
                <th className="py-2 pr-4 font-medium">{t("users.columns.eppn")}</th>
                <th className="py-2 pr-4 font-medium">{t("users.columns.role")}</th>
                <th className="py-2 pr-4 font-medium">{t("users.columns.ownerCap")}</th>
                <th className="py-2 pr-4 font-medium">{t("users.columns.status")}</th>
                <th className="py-2 font-medium">{t("users.columns.actions")}</th>
              </tr>
            </thead>
            <tbody>
              {filtered.map((u) => (
                <UserRow key={u.id} user={u} />
              ))}
            </tbody>
          </table>
        </div>
      </CardContent>
    </Card>
  )
}

function UserRow({ user }: { user: AdminUser }) {
  const { t } = useTranslation("admin")
  const formatError = useApiErrorMessage()
  const queryClient = useQueryClient()
  const invalidate = () =>
    queryClient.invalidateQueries({ queryKey: ["admin", "users"] })

  const roleMutation = useMutation({
    mutationFn: (role: string) =>
      api.put(`/admin/users/${user.id}/role`, { role }),
    onSuccess: invalidate,
  })

  const unlockMutation = useMutation({
    mutationFn: () => api.delete(`/admin/users/${user.id}/role-lock`),
    onSuccess: invalidate,
  })

  const suspendMutation = useMutation({
    mutationFn: (suspended: boolean) =>
      api.put(`/admin/users/${user.id}/suspended`, { suspended }),
    onSuccess: invalidate,
  })

  // Deletes today's usage_daily rows for this user so both per-course and
  // owner-aggregate quotas reset immediately, without waiting for UTC
  // midnight. Also invalidates the admin usage tab so numbers refresh.
  const resetUsageMutation = useMutation({
    mutationFn: () => api.delete(`/admin/users/${user.id}/daily-usage`),
    onSuccess: () => {
      invalidate()
      queryClient.invalidateQueries({ queryKey: ["admin", "usage"] })
    },
  })

  return (
    <tr className="border-b">
      <td className="py-2 pr-4">{user.display_name ?? "-"}</td>
      <td className="py-2 pr-4 font-mono text-xs">{user.eppn}</td>
      <td className="py-2 pr-4">
        {user.role === "admin" ? (
          <Badge>{t("users.roles.admin")}</Badge>
        ) : (
          <div className="flex items-center gap-2">
            <Select
              value={user.role}
              onValueChange={(v) => v && roleMutation.mutate(v)}
              disabled={roleMutation.isPending}
            >
              <SelectTrigger className="h-7 w-24 text-xs">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="student">{t("users.roles.student")}</SelectItem>
                <SelectItem value="teacher">{t("users.roles.teacher")}</SelectItem>
              </SelectContent>
            </Select>
            {user.role_manually_set && (
              <Badge
                variant="outline"
                className="h-6 px-2.5"
                title={t("users.lockedTitle")}
              >
                {t("users.locked")}
              </Badge>
            )}
          </div>
        )}
      </td>
      <td className="py-2 pr-4">
        <OwnerLimitInput user={user} />
      </td>
      <td className="py-2 pr-4">
        {user.suspended ? (
          <Badge variant="destructive">{t("users.status.suspended")}</Badge>
        ) : (
          <Badge variant="secondary">{t("users.status.active")}</Badge>
        )}
      </td>
      <td className="py-2">
        {user.role !== "admin" && (
          <div className="flex items-center gap-2">
            <Menu.Root>
              <Menu.Trigger
                render={
                  <Button
                    variant="ghost"
                    size="sm"
                    className="h-7 w-7 p-0"
                    aria-label={t("users.actionsLabel")}
                  >
                    <MoreHorizontalIcon className="size-4" />
                  </Button>
                }
              />
              <Menu.Portal>
                <Menu.Positioner
                  side="bottom"
                  align="end"
                  sideOffset={4}
                  className="isolate z-50"
                >
                  <Menu.Popup className="min-w-40 origin-(--transform-origin) rounded-lg bg-popover p-1 text-popover-foreground shadow-md ring-1 ring-foreground/10 data-open:animate-in data-open:fade-in-0 data-open:zoom-in-95 data-closed:animate-out data-closed:fade-out-0 data-closed:zoom-out-95">
                    {user.role_manually_set && (
                      <Menu.Item
                        className="relative flex cursor-default items-center rounded-md px-2 py-1.5 text-sm outline-hidden select-none data-highlighted:bg-accent data-highlighted:text-accent-foreground data-disabled:pointer-events-none data-disabled:opacity-50"
                        disabled={unlockMutation.isPending}
                        onClick={() => unlockMutation.mutate()}
                      >
                        {t("users.unlockRole")}
                      </Menu.Item>
                    )}
                    <Menu.Item
                      className="relative flex cursor-default items-center rounded-md px-2 py-1.5 text-sm outline-hidden select-none data-highlighted:bg-accent data-highlighted:text-accent-foreground data-disabled:pointer-events-none data-disabled:opacity-50"
                      disabled={resetUsageMutation.isPending}
                      onClick={() => resetUsageMutation.mutate()}
                    >
                      {t("users.resetDailyUsage")}
                    </Menu.Item>
                    <Menu.Item
                      className="relative flex cursor-default items-center rounded-md px-2 py-1.5 text-sm outline-hidden select-none data-highlighted:bg-accent data-highlighted:text-accent-foreground data-disabled:pointer-events-none data-disabled:opacity-50 data-[variant=destructive]:text-destructive data-[variant=destructive]:data-highlighted:bg-destructive/10"
                      data-variant={user.suspended ? undefined : "destructive"}
                      disabled={suspendMutation.isPending}
                      onClick={() => suspendMutation.mutate(!user.suspended)}
                    >
                      {user.suspended ? t("users.unsuspend") : t("users.suspend")}
                    </Menu.Item>
                  </Menu.Popup>
                </Menu.Positioner>
              </Menu.Portal>
            </Menu.Root>
            {suspendMutation.isError && (
              <span className="text-xs text-destructive">
                {formatError(suspendMutation.error)}
              </span>
            )}
            {resetUsageMutation.isError && (
              <span className="text-xs text-destructive">
                {formatError(resetUsageMutation.error)}
              </span>
            )}
          </div>
        )}
      </td>
    </tr>
  )
}

function OwnerLimitInput({ user }: { user: AdminUser }) {
  const { t } = useTranslation("admin")
  const { t: tCommon } = useTranslation("common")
  const queryClient = useQueryClient()
  const [draft, setDraft] = useState(String(user.owner_daily_token_limit))

  const mutation = useMutation({
    mutationFn: (limit: number) =>
      api.put(`/admin/users/${user.id}/owner-daily-token-limit`, { limit }),
    onSuccess: () =>
      queryClient.invalidateQueries({ queryKey: ["admin", "users"] }),
  })

  const dirty = draft !== String(user.owner_daily_token_limit)

  return (
    <div className="flex items-center gap-2">
      <input
        className="h-7 w-28 rounded border bg-background px-2 text-xs font-mono"
        value={draft}
        onChange={(e) => setDraft(e.target.value.replace(/[^0-9]/g, ""))}
        placeholder="0"
      />
      {dirty && (
        <Button
          size="sm"
          variant="outline"
          className="h-7 text-xs"
          onClick={() => {
            const n = Number(draft)
            if (Number.isFinite(n) && n >= 0) mutation.mutate(n)
          }}
          disabled={mutation.isPending}
        >
          {tCommon("actions.save")}
        </Button>
      )}
      {user.owner_daily_token_limit === 0 && !dirty && (
        <span className="text-xs text-muted-foreground">{t("users.ownerLimit.unlimited")}</span>
      )}
    </div>
  )
}
