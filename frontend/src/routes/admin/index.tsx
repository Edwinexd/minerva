import { createFileRoute } from "@tanstack/react-router"
import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query"
import { adminUsersQuery, adminUsageQuery, coursesQuery } from "@/lib/queries"
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
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs"
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

export const Route = createFileRoute("/admin/")({
  component: AdminDashboard,
})

function AdminDashboard() {
  return (
    <div className="space-y-6">
      <h2 className="text-2xl font-bold tracking-tight">Platform Admin</h2>

      <Tabs defaultValue="usage">
        <TabsList>
          <TabsTrigger value="usage">Platform Usage</TabsTrigger>
          <TabsTrigger value="users">User Management</TabsTrigger>
        </TabsList>

        <TabsContent value="usage" className="mt-4">
          <PlatformUsagePanel />
        </TabsContent>

        <TabsContent value="users" className="mt-4">
          <UserManagementPanel />
        </TabsContent>
      </Tabs>
    </div>
  )
}

function PlatformUsagePanel() {
  const { data: usage, isLoading: usageLoading } = useQuery(adminUsageQuery)
  const { data: courses, isLoading: coursesLoading } = useQuery(coursesQuery)
  const { data: users } = useQuery(adminUsersQuery)

  if (usageLoading || coursesLoading) {
    return (
      <div className="space-y-4">
        {Array.from({ length: 3 }).map((_, i) => (
          <Skeleton key={i} className="h-32 w-full" />
        ))}
      </div>
    )
  }

  if (!usage || !courses) return null

  const courseMap = new Map(courses.map((c) => [c.id, c]))
  const userMap = new Map((users ?? []).map((u) => [u.id, u]))

  // Aggregate usage per course
  const byCourse = new Map<string, { tokens: number; requests: number }>()
  for (const row of usage) {
    const existing = byCourse.get(row.course_id) ?? { tokens: 0, requests: 0 }
    existing.tokens += row.prompt_tokens + row.completion_tokens
    existing.requests += row.request_count
    byCourse.set(row.course_id, existing)
  }

  const totalTokens = usage.reduce(
    (sum, r) => sum + r.prompt_tokens + r.completion_tokens,
    0,
  )
  const totalRequests = usage.reduce((sum, r) => sum + r.request_count, 0)

  // Per-user totals across all courses
  const byUser = new Map<string, { tokens: number; requests: number }>()
  for (const row of usage) {
    const existing = byUser.get(row.user_id) ?? { tokens: 0, requests: 0 }
    existing.tokens += row.prompt_tokens + row.completion_tokens
    existing.requests += row.request_count
    byUser.set(row.user_id, existing)
  }

  const sortedUsers = [...byUser.entries()].sort(
    (a, b) => b[1].tokens - a[1].tokens,
  )

  return (
    <div className="space-y-6">
      <div className="grid gap-4 md:grid-cols-3">
        <Card>
          <CardHeader className="pb-2">
            <CardDescription>Total Tokens</CardDescription>
            <CardTitle className="text-2xl">{totalTokens.toLocaleString()}</CardTitle>
          </CardHeader>
        </Card>
        <Card>
          <CardHeader className="pb-2">
            <CardDescription>Total Requests</CardDescription>
            <CardTitle className="text-2xl">{totalRequests.toLocaleString()}</CardTitle>
          </CardHeader>
        </Card>
        <Card>
          <CardHeader className="pb-2">
            <CardDescription>Active Courses</CardDescription>
            <CardTitle className="text-2xl">{courses.length}</CardTitle>
          </CardHeader>
        </Card>
      </div>

      <Card>
        <CardHeader>
          <CardTitle>Usage by Course</CardTitle>
        </CardHeader>
        <CardContent>
          {byCourse.size === 0 ? (
            <p className="text-muted-foreground">No usage data yet.</p>
          ) : (
            <div className="overflow-x-auto">
              <table className="w-full text-sm">
                <thead>
                  <tr className="border-b text-left">
                    <th className="py-2 pr-4 font-medium">Course</th>
                    <th className="py-2 pr-4 font-medium text-right">Tokens</th>
                    <th className="py-2 pr-4 font-medium text-right">Requests</th>
                    <th className="py-2 font-medium text-right">Limit</th>
                  </tr>
                </thead>
                <tbody>
                  {[...byCourse.entries()]
                    .sort((a, b) => b[1].tokens - a[1].tokens)
                    .map(([courseId, stats]) => {
                      const course = courseMap.get(courseId)
                      return (
                        <tr key={courseId} className="border-b">
                          <td className="py-2 pr-4">
                            {course?.name ?? courseId.slice(0, 8)}
                          </td>
                          <td className="py-2 pr-4 text-right font-mono">
                            {stats.tokens.toLocaleString()}
                          </td>
                          <td className="py-2 pr-4 text-right font-mono">
                            {stats.requests.toLocaleString()}
                          </td>
                          <td className="py-2 text-right font-mono">
                            {course?.daily_token_limit
                              ? `${course.daily_token_limit.toLocaleString()}/day`
                              : "unlimited"}
                          </td>
                        </tr>
                      )
                    })}
                </tbody>
              </table>
            </div>
          )}
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle>Top Users by Token Usage</CardTitle>
        </CardHeader>
        <CardContent>
          {sortedUsers.length === 0 ? (
            <p className="text-muted-foreground">No usage data yet.</p>
          ) : (
            <div className="overflow-x-auto">
              <table className="w-full text-sm">
                <thead>
                  <tr className="border-b text-left">
                    <th className="py-2 pr-4 font-medium">User</th>
                    <th className="py-2 pr-4 font-medium text-right">Tokens</th>
                    <th className="py-2 font-medium text-right">Requests</th>
                  </tr>
                </thead>
                <tbody>
                  {sortedUsers.slice(0, 50).map(([userId, stats]) => {
                    const u = userMap.get(userId)
                    return (
                      <tr key={userId} className="border-b">
                        <td className="py-2 pr-4">
                          {u?.display_name ?? u?.eppn ?? userId.slice(0, 8)}
                        </td>
                        <td className="py-2 pr-4 text-right font-mono">
                          {stats.tokens.toLocaleString()}
                        </td>
                        <td className="py-2 text-right font-mono">
                          {stats.requests.toLocaleString()}
                        </td>
                      </tr>
                    )
                  })}
                </tbody>
              </table>
            </div>
          )}
        </CardContent>
      </Card>
    </div>
  )
}

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
