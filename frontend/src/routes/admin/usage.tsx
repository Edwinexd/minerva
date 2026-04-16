import { createFileRoute } from "@tanstack/react-router"
import { useQuery } from "@tanstack/react-query"
import { useTranslation } from "react-i18next"
import { adminUsersQuery, adminUsageQuery, coursesQuery } from "@/lib/queries"
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card"
import { Skeleton } from "@/components/ui/skeleton"

export const Route = createFileRoute("/admin/usage")({
  component: PlatformUsagePanel,
})

function PlatformUsagePanel() {
  const { t } = useTranslation("admin")
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
            <CardDescription>{t("usage.totalTokens")}</CardDescription>
            <CardTitle className="text-2xl">{totalTokens.toLocaleString()}</CardTitle>
          </CardHeader>
        </Card>
        <Card>
          <CardHeader className="pb-2">
            <CardDescription>{t("usage.totalRequests")}</CardDescription>
            <CardTitle className="text-2xl">{totalRequests.toLocaleString()}</CardTitle>
          </CardHeader>
        </Card>
        <Card>
          <CardHeader className="pb-2">
            <CardDescription>{t("usage.activeCourses")}</CardDescription>
            <CardTitle className="text-2xl">{courses.length}</CardTitle>
          </CardHeader>
        </Card>
      </div>

      <Card>
        <CardHeader>
          <CardTitle>{t("usage.byCourseTitle")}</CardTitle>
        </CardHeader>
        <CardContent>
          {byCourse.size === 0 ? (
            <p className="text-muted-foreground">{t("usage.byCourseEmpty")}</p>
          ) : (
            <div className="overflow-x-auto">
              <table className="w-full text-sm">
                <thead>
                  <tr className="border-b text-left">
                    <th className="py-2 pr-4 font-medium">{t("usage.columns.course")}</th>
                    <th className="py-2 pr-4 font-medium text-right">{t("usage.columns.tokens")}</th>
                    <th className="py-2 pr-4 font-medium text-right">{t("usage.columns.requests")}</th>
                    <th className="py-2 font-medium text-right">{t("usage.columns.limit")}</th>
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
                              ? t("usage.limit.perDay", {
                                  value: course.daily_token_limit.toLocaleString(),
                                })
                              : t("usage.limit.unlimited")}
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
          <CardTitle>{t("usage.topUsersTitle")}</CardTitle>
        </CardHeader>
        <CardContent>
          {sortedUsers.length === 0 ? (
            <p className="text-muted-foreground">{t("usage.topUsersEmpty")}</p>
          ) : (
            <div className="overflow-x-auto">
              <table className="w-full text-sm">
                <thead>
                  <tr className="border-b text-left">
                    <th className="py-2 pr-4 font-medium">{t("usage.columns.user")}</th>
                    <th className="py-2 pr-4 font-medium text-right">{t("usage.columns.tokens")}</th>
                    <th className="py-2 font-medium text-right">{t("usage.columns.requests")}</th>
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
