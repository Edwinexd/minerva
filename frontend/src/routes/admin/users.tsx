import { createFileRoute } from "@tanstack/react-router"
import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query"
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
          Manage user roles and account status
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

  const roleMutation = useMutation({
    mutationFn: (role: string) =>
      api.put(`/admin/users/${user.id}/role`, { role }),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["admin", "users"] })
    },
  })

  const suspendMutation = useMutation({
    mutationFn: (suspended: boolean) =>
      api.put(`/admin/users/${user.id}/suspended`, { suspended }),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["admin", "users"] })
    },
  })

  return (
    <tr className="border-b">
      <td className="py-2 pr-4">{user.display_name ?? "-"}</td>
      <td className="py-2 pr-4 font-mono text-xs">{user.eppn}</td>
      <td className="py-2 pr-4">
        {user.role === "admin" ? (
          <Badge>admin</Badge>
        ) : (
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
        )}
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
          <Button
            variant={user.suspended ? "outline" : "destructive"}
            size="sm"
            className="h-7 text-xs"
            onClick={() => suspendMutation.mutate(!user.suspended)}
            disabled={suspendMutation.isPending}
          >
            {user.suspended ? "Unsuspend" : "Suspend"}
          </Button>
        )}
        {suspendMutation.isError && (
          <span className="ml-2 text-xs text-destructive">
            {suspendMutation.error.message}
          </span>
        )}
      </td>
    </tr>
  )
}
