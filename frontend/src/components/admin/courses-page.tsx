import { Link } from "@tanstack/react-router"
import { RelativeTime } from "@/components/relative-time"
import { useQuery } from "@tanstack/react-query"
import { useTranslation } from "react-i18next"
import { adminUsersQuery, coursesQuery } from "@/lib/queries"
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card"
import { Badge } from "@/components/ui/badge"
import { Skeleton } from "@/components/ui/skeleton"
import { useState } from "react"

export function CourseManagementPanel() {
  const { t } = useTranslation("admin")
  const { data: courses, isLoading: coursesLoading } = useQuery(coursesQuery)
  const { data: users } = useQuery(adminUsersQuery)
  const [filter, setFilter] = useState("")

  if (coursesLoading) {
    return (
      <div className="space-y-2">
        {Array.from({ length: 5 }).map((_, i) => (
          <Skeleton key={i} className="h-14 w-full" />
        ))}
      </div>
    )
  }

  if (!courses) return null

  const userMap = new Map((users ?? []).map((u) => [u.id, u]))

  const filtered = filter
    ? courses.filter((c) => {
        const owner = userMap.get(c.owner_id)
        const ownerLabel = owner?.display_name ?? owner?.eppn ?? c.owner_id
        return (
          c.name.toLowerCase().includes(filter.toLowerCase()) ||
          ownerLabel.toLowerCase().includes(filter.toLowerCase())
        )
      })
    : courses

  return (
    <Card>
      <CardHeader>
        <CardTitle>{t("courses.title", { total: courses.length })}</CardTitle>
        <CardDescription>{t("courses.description")}</CardDescription>
        <input
          className="mt-2 w-full max-w-sm rounded border bg-background px-3 py-1.5 text-sm"
          placeholder={t("courses.filterPlaceholder")}
          value={filter}
          onChange={(e) => setFilter(e.target.value)}
        />
      </CardHeader>
      <CardContent>
        <div className="overflow-x-auto">
          <table className="w-full text-sm">
            <thead>
              <tr className="border-b text-left">
                <th className="py-2 pr-4 font-medium">{t("courses.columns.course")}</th>
                <th className="py-2 pr-4 font-medium">{t("courses.columns.owner")}</th>
                <th className="py-2 pr-4 font-medium">{t("courses.columns.status")}</th>
                <th className="py-2 pr-4 font-medium">{t("courses.columns.tokenLimit")}</th>
                <th className="py-2 pr-4 font-medium">{t("courses.columns.created")}</th>
                <th className="py-2 font-medium">{t("courses.columns.settings")}</th>
              </tr>
            </thead>
            <tbody>
              {filtered.map((course) => {
                const owner = userMap.get(course.owner_id)
                const ownerLabel =
                  owner?.display_name ?? owner?.eppn ?? course.owner_id.slice(0, 8)
                return (
                  <tr key={course.id} className="border-b">
                    <td className="py-2 pr-4 font-medium">{course.name}</td>
                    <td className="py-2 pr-4 text-muted-foreground">
                      {ownerLabel}
                    </td>
                    <td className="py-2 pr-4">
                      {course.active ? (
                        <Badge variant="secondary">{t("courses.status.active")}</Badge>
                      ) : (
                        <Badge variant="outline">{t("courses.status.archived")}</Badge>
                      )}
                    </td>
                    <td className="py-2 pr-4 font-mono">
                      {course.daily_token_limit === 0
                        ? t("courses.tokenLimitUnlimited")
                        : course.daily_token_limit.toLocaleString()}
                    </td>
                    <td className="py-2 pr-4 text-muted-foreground">
                      <RelativeTime date={course.created_at} />
                    </td>
                    <td className="py-2">
                      <Link
                        to="/teacher/courses/$courseId/config"
                        params={{ courseId: course.id }}
                        className="text-primary underline-offset-4 hover:underline"
                      >
                        {t("courses.settingsLink")}
                      </Link>
                    </td>
                  </tr>
                )
              })}
            </tbody>
          </table>
          {filtered.length === 0 && (
            <p className="py-4 text-center text-sm text-muted-foreground">
              {t("courses.empty")}
            </p>
          )}
        </div>
      </CardContent>
    </Card>
  )
}
