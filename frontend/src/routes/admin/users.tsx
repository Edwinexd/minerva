import { createFileRoute } from "@tanstack/react-router"
import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query"
import { Menu } from "@base-ui/react/menu"
import { MoreHorizontalIcon } from "lucide-react"
import { adminUsersQuery } from "@/lib/queries"
import { api } from "@/lib/api"
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

export const Route = createFileRoute("/admin/users")({
  component: UserManagementPanel,
})

function UserManagementPanel() {
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
        <CardTitle>Users ({users.length})</CardTitle>
        <CardDescription>
          Manage user roles, daily AI spending limits, and account status.
          Setting a role manually locks it from auto-promotion rules.
        </CardDescription>
        <input
          className="mt-2 w-full max-w-sm rounded border bg-background px-3 py-1.5 text-sm"
          placeholder="Filter by name or eppn..."
          value={filter}
          onChange={(e) => setFilter(e.target.value)}
        />
      </CardHeader>
      <CardContent>
        <div className="overflow-x-auto">
          <table className="w-full text-sm">
            <thead>
              <tr className="border-b text-left">
                <th className="py-2 pr-4 font-medium">User</th>
                <th className="py-2 pr-4 font-medium">eppn</th>
                <th className="py-2 pr-4 font-medium">Role</th>
                <th className="py-2 pr-4 font-medium">Owner cap (tok/day)</th>
                <th className="py-2 pr-4 font-medium">Status</th>
                <th className="py-2 font-medium">Actions</th>
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

  return (
    <tr className="border-b">
      <td className="py-2 pr-4">{user.display_name ?? "-"}</td>
      <td className="py-2 pr-4 font-mono text-xs">{user.eppn}</td>
      <td className="py-2 pr-4">
        {user.role === "admin" ? (
          <Badge>admin</Badge>
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
                <SelectItem value="student">student</SelectItem>
                <SelectItem value="teacher">teacher</SelectItem>
              </SelectContent>
            </Select>
            {user.role_manually_set && (
              <Badge
                variant="outline"
                className="h-6 px-2.5"
                title="Locked from rule auto-promotion"
              >
                locked
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
          <Badge variant="destructive">suspended</Badge>
        ) : (
          <Badge variant="secondary">active</Badge>
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
                    aria-label="User actions"
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
                        Unlock role
                      </Menu.Item>
                    )}
                    <Menu.Item
                      className="relative flex cursor-default items-center rounded-md px-2 py-1.5 text-sm outline-hidden select-none data-highlighted:bg-accent data-highlighted:text-accent-foreground data-disabled:pointer-events-none data-disabled:opacity-50 data-[variant=destructive]:text-destructive data-[variant=destructive]:data-highlighted:bg-destructive/10"
                      data-variant={user.suspended ? undefined : "destructive"}
                      disabled={suspendMutation.isPending}
                      onClick={() => suspendMutation.mutate(!user.suspended)}
                    >
                      {user.suspended ? "Unsuspend" : "Suspend"}
                    </Menu.Item>
                  </Menu.Popup>
                </Menu.Positioner>
              </Menu.Portal>
            </Menu.Root>
            {suspendMutation.isError && (
              <span className="text-xs text-destructive">
                {suspendMutation.error.message}
              </span>
            )}
          </div>
        )}
      </td>
    </tr>
  )
}

function OwnerLimitInput({ user }: { user: AdminUser }) {
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
          Save
        </Button>
      )}
      {user.owner_daily_token_limit === 0 && !dirty && (
        <span className="text-xs text-muted-foreground">unlimited</span>
      )}
    </div>
  )
}
