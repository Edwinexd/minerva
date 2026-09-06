import { useState } from "react"
import { Link } from "@tanstack/react-router"
import { useQuery } from "@tanstack/react-query"
import { useTranslation } from "react-i18next"
import { adminUserUsageQuery } from "@/lib/queries"
import { formatUsd } from "@/lib/currency"
import { peakSpendDay } from "@/lib/owner-usage"
import {
  OwnerUsageSkeleton,
  OwnerUsageView,
} from "@/components/teacher/owner-usage-page"
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card"

/**
 * One account's AI spend, as an admin sees it: the same panels the
 * teacher gets on their own page, minus the limit-increase draft (an
 * admin does not mail themselves) and plus the link to the dial that
 * actually changes the limit.
 */
export function AdminUserUsagePage({
  useParams,
}: {
  useParams: () => { userId: string }
}) {
  const { userId } = useParams()
  const { t } = useTranslation("admin")
  const [days, setDays] = useState<number>(30)
  const { data: usage, isLoading } = useQuery(adminUserUsageQuery(userId, days))

  if (isLoading) return <OwnerUsageSkeleton />
  if (!usage) return null

  const name = usage.owner_display_name || usage.owner_eppn

  return (
    <div className="space-y-6">
      <div>
        <h2 className="text-lg font-semibold">
          {t("userUsage.title", { name })}
        </h2>
        <p className="text-sm text-muted-foreground font-mono">
          {usage.owner_eppn}
        </p>
        <p className="mt-1 text-sm">
          <Link to="/admin/usage" className="underline hover:text-foreground">
            {t("userUsage.backToUsage")}
          </Link>
        </p>
      </div>

      <OwnerUsageView
        usage={usage}
        days={days}
        onDaysChange={setDays}
        copy={{
          description: t("userUsage.description", { name }),
          capReached: t("userUsage.capReached", { name }),
          capNear: t("userUsage.capNear", { name }),
          noCourses: t("userUsage.noCourses", { name }),
          limitUsed: t("userUsage.limitUsed", {
            percent: Math.round(
              usage.daily_cost_limit_usd > 0
                ? (usage.spend_today_usd / usage.daily_cost_limit_usd) * 100
                : 0,
            ),
            limit: formatUsd(usage.daily_cost_limit_usd),
          }),
          limitScope: t("userUsage.limitScope", { name }),
          byCourseDescription: t("userUsage.byCourseDescription", { name }),
        }}
        actionCard={
          <Card>
            <CardHeader>
              <CardTitle>{t("userUsage.limitCard.title")}</CardTitle>
              <CardDescription>
                {usage.daily_cost_limit_usd > 0
                  ? t("userUsage.limitCard.current", {
                      value: formatUsd(usage.daily_cost_limit_usd),
                    })
                  : t("userUsage.limitCard.unlimited")}
              </CardDescription>
            </CardHeader>
            <CardContent className="space-y-2 text-sm text-muted-foreground">
              <p>
                {t("userUsage.limitCard.peak", {
                  value: formatUsd(peakSpendDay(usage)),
                  days: usage.window_days,
                })}
              </p>
              <p>
                <Link to="/admin/users" className="underline hover:text-foreground">
                  {t("userUsage.limitCard.edit")}
                </Link>
              </p>
            </CardContent>
          </Card>
        }
      />
    </div>
  )
}
