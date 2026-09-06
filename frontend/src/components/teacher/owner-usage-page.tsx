import { useState } from "react"
import { Link } from "@tanstack/react-router"
import { useQuery } from "@tanstack/react-query"
import { useTranslation } from "react-i18next"
import { teacherUsageQuery, userQuery } from "@/lib/queries"
import { formatUsd } from "@/lib/currency"
import { useCopyFeedback } from "@/lib/use-copy-feedback"
import type { OwnerCourseUsage, OwnerUsage } from "@/lib/types"
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card"
import { Badge } from "@/components/ui/badge"
import { Button, buttonVariants } from "@/components/ui/button"
import { Skeleton } from "@/components/ui/skeleton"
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select"

/// Where limit-increase requests go. Same address as the 429 body the
/// students see when a cap is hit.
const SUPPORT_EMAIL = "lambda@dsv.su.se"

/// Providers whose spend is real money off a unit's budget, so a limit
/// increase needs that unit to approve it. Anything else (self-hosted)
/// only costs cluster time.
const BUDGETED_PROVIDERS = new Set(["cerebras"])

const WINDOW_OPTIONS = [7, 30, 90] as const

function formatTokens(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`
  if (n >= 1_000) return `${(n / 1_000).toFixed(1)}k`
  return n.toString()
}

/// Course label for the table and the mail draft: "Programming 1
/// (PROG1)" when a code exists, bare name otherwise.
function courseLabel(course: OwnerCourseUsage): string {
  return course.course_code ? `${course.name} (${course.course_code})` : course.name
}

export function OwnerUsagePage() {
  const { t } = useTranslation("teacher")
  const [days, setDays] = useState<number>(30)
  const { data: user } = useQuery(userQuery)
  const { data: usage, isLoading } = useQuery(teacherUsageQuery(days))

  if (isLoading) {
    return (
      <div className="space-y-4">
        {Array.from({ length: 3 }).map((_, i) => (
          <Skeleton key={i} className="h-32 w-full" />
        ))}
      </div>
    )
  }
  if (!usage) return null

  const limit = usage.daily_cost_limit_usd
  const usedFraction = limit > 0 ? usage.spend_today_usd / limit : 0
  const usedPercent = Math.round(usedFraction * 100)
  const capReached = limit > 0 && usage.spend_today_usd >= limit
  const capNear = !capReached && usedFraction >= 0.8
  // The busiest day in the window is what a limit request should be
  // sized against: a cap set to the average would still cut out on the
  // day a class actually uses the tool.
  const peakDay = usage.daily.reduce(
    (max, d) => Math.max(max, d.chat_spend_usd + d.pipeline_spend_usd),
    0,
  )

  return (
    <div className="space-y-6">
      <div className="flex flex-wrap items-end justify-between gap-3">
        <p className="text-sm text-muted-foreground max-w-3xl">
          {t("ownerUsage.description")}
        </p>
        <Select
          value={String(days)}
          onValueChange={(value) => value && setDays(Number(value))}
        >
          <SelectTrigger className="w-44" aria-label={t("ownerUsage.windowLabel")}>
            <SelectValue>{t("ownerUsage.windowOption", { days })}</SelectValue>
          </SelectTrigger>
          <SelectContent>
            {WINDOW_OPTIONS.map((option) => (
              <SelectItem key={option} value={String(option)}>
                {t("ownerUsage.windowOption", { days: option })}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      </div>

      <div className="grid gap-4 md:grid-cols-2 lg:grid-cols-4">
        <Card>
          <CardHeader className="pb-2">
            <CardDescription>{t("ownerUsage.spendToday")}</CardDescription>
            <CardTitle className="text-2xl">
              {formatUsd(usage.spend_today_usd)}
            </CardTitle>
            {limit > 0 && (
              <>
                <progress
                  value={Math.min(usedPercent, 100)}
                  max={100}
                  aria-label={t("ownerUsage.limitUsedLabel")}
                  className={`mt-1 h-1.5 w-full overflow-hidden rounded-full bg-muted [&::-webkit-progress-bar]:bg-muted ${
                    capReached
                      ? "[&::-moz-progress-bar]:bg-destructive [&::-webkit-progress-value]:bg-destructive"
                      : "[&::-moz-progress-bar]:bg-primary [&::-webkit-progress-value]:bg-primary"
                  }`}
                />
                <p className="text-xs text-muted-foreground">
                  {t("ownerUsage.limitUsed", {
                    percent: usedPercent,
                    limit: formatUsd(limit),
                  })}
                </p>
              </>
            )}
          </CardHeader>
        </Card>
        <Card>
          <CardHeader className="pb-2">
            <CardDescription>{t("ownerUsage.dailyLimit")}</CardDescription>
            <CardTitle className="text-2xl">
              {limit > 0 ? formatUsd(limit) : t("ownerUsage.unlimited")}
            </CardTitle>
            <p className="text-xs text-muted-foreground">
              {t("ownerUsage.limitScope")}
            </p>
          </CardHeader>
        </Card>
        <Card>
          <CardHeader className="pb-2">
            <CardDescription>
              {t("ownerUsage.windowSpend", { days: usage.window_days })}
            </CardDescription>
            <CardTitle className="text-2xl">
              {formatUsd(usage.window_spend_usd)}
            </CardTitle>
            <p className="text-xs text-muted-foreground">
              {t("ownerUsage.spendSplit", {
                chat: formatUsd(usage.window_chat_spend_usd),
                pipeline: formatUsd(usage.window_pipeline_spend_usd),
              })}
            </p>
          </CardHeader>
        </Card>
        <Card>
          <CardHeader className="pb-2">
            <CardDescription>{t("ownerUsage.peakDay")}</CardDescription>
            <CardTitle className="text-2xl">{formatUsd(peakDay)}</CardTitle>
            <p className="text-xs text-muted-foreground">
              {t("ownerUsage.windowRequests", {
                count: usage.window_requests,
              })}
            </p>
          </CardHeader>
        </Card>
      </div>

      {capReached && (
        <p className="rounded-md border border-destructive/50 bg-destructive/10 px-4 py-3 text-sm">
          {t("ownerUsage.capReached")}
        </p>
      )}
      {capNear && (
        <p className="rounded-md border bg-muted/40 px-4 py-3 text-sm text-muted-foreground">
          {t("ownerUsage.capNear")}
        </p>
      )}

      <IncreaseRequestCard
        usage={usage}
        eppn={user?.eppn ?? ""}
        displayName={user?.display_name ?? null}
        peakDay={peakDay}
      />

      <Card>
        <CardHeader>
          <CardTitle>{t("ownerUsage.byCourse.title")}</CardTitle>
          <CardDescription>
            {t("ownerUsage.byCourse.description")}
          </CardDescription>
        </CardHeader>
        <CardContent>
          {usage.courses.length === 0 ? (
            <p className="text-sm text-muted-foreground">
              {t("ownerUsage.byCourse.empty")}
            </p>
          ) : (
            <div className="overflow-x-auto">
              <table className="w-full text-sm">
                <thead>
                  <tr className="border-b text-left">
                    <th className="py-2 pr-4 font-medium">
                      {t("ownerUsage.byCourse.colCourse")}
                    </th>
                    <th className="py-2 pr-4 font-medium">
                      {t("ownerUsage.byCourse.colModel")}
                    </th>
                    <th className="py-2 pr-4 font-medium text-right">
                      {t("ownerUsage.byCourse.colToday")}
                    </th>
                    <th className="py-2 pr-4 font-medium text-right">
                      {t("ownerUsage.byCourse.colWindow", {
                        days: usage.window_days,
                      })}
                    </th>
                    <th className="py-2 pr-4 font-medium text-right">
                      {t("ownerUsage.byCourse.colStudents")}
                    </th>
                    <th className="py-2 font-medium text-right">
                      {t("ownerUsage.byCourse.colStudentLimit")}
                    </th>
                  </tr>
                </thead>
                <tbody>
                  {usage.courses.map((course) => (
                    <tr key={course.id} className="border-b align-top">
                      <td className="py-2 pr-4">
                        {course.active ? (
                          <Link
                            to="/teacher/courses/$courseId/usage"
                            params={{ courseId: course.id }}
                            className="underline hover:text-foreground"
                          >
                            {courseLabel(course)}
                          </Link>
                        ) : (
                          <span>{courseLabel(course)}</span>
                        )}
                        {course.semester_label && (
                          <span className="block text-[10px] text-muted-foreground">
                            {course.semester_label}
                          </span>
                        )}
                        {!course.active && (
                          <Badge variant="outline" className="mt-1">
                            {t("ownerUsage.byCourse.archived")}
                          </Badge>
                        )}
                      </td>
                      <td className="py-2 pr-4">
                        <span className="font-mono text-xs">
                          {course.model ?? t("ownerUsage.unknown")}
                        </span>
                        {course.provider && (
                          <span className="block text-[10px] text-muted-foreground">
                            {course.provider}
                          </span>
                        )}
                      </td>
                      <td className="py-2 pr-4 text-right font-mono">
                        {formatUsd(course.spend_today_usd)}
                      </td>
                      <td className="py-2 pr-4 text-right font-mono">
                        {formatUsd(course.window_spend_usd)}
                        <span className="block text-[10px] font-normal text-muted-foreground">
                          {t("ownerUsage.byCourse.tokens", {
                            tokens: formatTokens(
                              course.window_prompt_tokens +
                                course.window_completion_tokens,
                            ),
                            requests: course.window_requests,
                          })}
                        </span>
                      </td>
                      <td className="py-2 pr-4 text-right font-mono">
                        {course.window_active_students}
                        <span className="block text-[10px] font-normal text-muted-foreground">
                          {t("ownerUsage.byCourse.enrolled", {
                            count: course.student_count,
                          })}
                        </span>
                      </td>
                      <td className="py-2 text-right font-mono">
                        {course.student_daily_cost_limit_usd > 0
                          ? formatUsd(course.student_daily_cost_limit_usd)
                          : t("ownerUsage.unlimited")}
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          )}
        </CardContent>
      </Card>

      <div className="grid gap-4 lg:grid-cols-2">
        <Card>
          <CardHeader>
            <CardTitle>{t("ownerUsage.providers.title")}</CardTitle>
            <CardDescription>
              {t("ownerUsage.providers.description")}
            </CardDescription>
          </CardHeader>
          <CardContent>
            {usage.providers.length === 0 ? (
              <p className="text-sm text-muted-foreground">
                {t("ownerUsage.empty")}
              </p>
            ) : (
              <table className="w-full text-sm">
                <thead>
                  <tr className="border-b text-left">
                    <th className="py-2 pr-4 font-medium">
                      {t("ownerUsage.providers.colProvider")}
                    </th>
                    <th className="py-2 pr-4 font-medium text-right">
                      {t("ownerUsage.providers.colSpend")}
                    </th>
                    <th className="py-2 font-medium text-right">
                      {t("ownerUsage.providers.colTokens")}
                    </th>
                  </tr>
                </thead>
                <tbody>
                  {usage.providers.map((provider) => (
                    <tr key={provider.provider} className="border-b">
                      <td className="py-2 pr-4">
                        {provider.provider}
                        {BUDGETED_PROVIDERS.has(provider.provider) && (
                          <Badge variant="secondary" className="ml-2">
                            {t("ownerUsage.providers.budgeted")}
                          </Badge>
                        )}
                      </td>
                      <td className="py-2 pr-4 text-right font-mono">
                        {formatUsd(provider.window_spend_usd)}
                      </td>
                      <td className="py-2 text-right font-mono">
                        {formatTokens(
                          provider.window_prompt_tokens +
                            provider.window_completion_tokens,
                        )}
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            )}
          </CardContent>
        </Card>

        <Card>
          <CardHeader>
            <CardTitle>{t("ownerUsage.daily.title")}</CardTitle>
            <CardDescription>
              {t("ownerUsage.daily.description", { days: usage.window_days })}
            </CardDescription>
          </CardHeader>
          <CardContent>
            {usage.daily.length === 0 ? (
              <p className="text-sm text-muted-foreground">
                {t("ownerUsage.empty")}
              </p>
            ) : (
              <div className="max-h-72 overflow-y-auto">
                <table className="w-full text-sm">
                  <thead>
                    <tr className="border-b text-left">
                      <th className="py-2 pr-4 font-medium">
                        {t("ownerUsage.daily.colDate")}
                      </th>
                      <th className="py-2 pr-4 font-medium text-right">
                        {t("ownerUsage.daily.colChat")}
                      </th>
                      <th className="py-2 pr-4 font-medium text-right">
                        {t("ownerUsage.daily.colPipeline")}
                      </th>
                      <th className="py-2 font-medium text-right">
                        {t("ownerUsage.daily.colTotal")}
                      </th>
                    </tr>
                  </thead>
                  <tbody>
                    {[...usage.daily].reverse().map((day) => (
                      <tr key={day.date} className="border-b">
                        <td className="py-2 pr-4">{day.date}</td>
                        <td className="py-2 pr-4 text-right font-mono">
                          {formatUsd(day.chat_spend_usd)}
                        </td>
                        <td className="py-2 pr-4 text-right font-mono">
                          {formatUsd(day.pipeline_spend_usd)}
                        </td>
                        <td className="py-2 text-right font-mono">
                          {formatUsd(day.chat_spend_usd + day.pipeline_spend_usd)}
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            )}
          </CardContent>
        </Card>
      </div>
    </div>
  )
}

/**
 * What to send when the daily cap is too low, plus a pre-filled draft.
 * The fields are the ones the request is actually decided on: who is
 * asking, which unit carries the cost, which courses, and how much is
 * already being spent. Everything except the unit and the requested
 * figure is filled in from the data on this page, so the teacher is
 * left with two blanks rather than a blank page.
 */
function IncreaseRequestCard({
  usage,
  eppn,
  displayName,
  peakDay,
}: {
  usage: OwnerUsage
  eppn: string
  displayName: string | null
  peakDay: number
}) {
  const { t } = useTranslation("teacher")
  const { copiedKey, copy } = useCopyFeedback()

  const budgetedProviders = usage.providers
    .map((p) => p.provider)
    .filter((provider) => BUDGETED_PROVIDERS.has(provider))
  const courseList = usage.courses
    .filter((c) => c.active)
    .map((c) => courseLabel(c))
    .join(", ")

  const draft = [
    t("ownerUsage.increase.draft.name", { name: displayName || eppn }),
    t("ownerUsage.increase.draft.eppn", { eppn }),
    t("ownerUsage.increase.draft.unit"),
    t("ownerUsage.increase.draft.courses", {
      courses: courseList || t("ownerUsage.increase.draft.noCourses"),
    }),
    t("ownerUsage.increase.draft.currentLimit", {
      limit:
        usage.daily_cost_limit_usd > 0
          ? formatUsd(usage.daily_cost_limit_usd)
          : t("ownerUsage.unlimited"),
    }),
    t("ownerUsage.increase.draft.peak", {
      peak: formatUsd(peakDay),
      days: usage.window_days,
    }),
    t("ownerUsage.increase.draft.requested"),
    t("ownerUsage.increase.draft.reason"),
    budgetedProviders.length > 0
      ? t("ownerUsage.increase.draft.budgetedProviders", {
          providers: budgetedProviders.join(", "),
        })
      : t("ownerUsage.increase.draft.selfHostedOnly"),
  ].join("\n")

  const subject = t("ownerUsage.increase.subject", { eppn })
  const mailto = `mailto:${SUPPORT_EMAIL}?subject=${encodeURIComponent(subject)}&body=${encodeURIComponent(draft)}`

  return (
    <Card>
      <CardHeader>
        <CardTitle>{t("ownerUsage.increase.title")}</CardTitle>
        <CardDescription>{t("ownerUsage.increase.lead")}</CardDescription>
      </CardHeader>
      <CardContent className="space-y-4">
        <ul className="list-disc pl-5 space-y-1 text-sm text-muted-foreground">
          <li>{t("ownerUsage.increase.items.identity")}</li>
          <li>{t("ownerUsage.increase.items.unit")}</li>
          <li>{t("ownerUsage.increase.items.courses")}</li>
          <li>{t("ownerUsage.increase.items.amount")}</li>
        </ul>

        {budgetedProviders.length > 0 && (
          <p className="rounded-md border bg-muted/40 px-4 py-3 text-sm">
            {t("ownerUsage.increase.budgetNote", {
              providers: budgetedProviders.join(", "),
            })}
          </p>
        )}

        <pre className="overflow-x-auto rounded-md border bg-muted/40 p-3 text-xs whitespace-pre-wrap">
          {draft}
        </pre>

        <div className="flex flex-wrap gap-2">
          <a href={mailto} className={buttonVariants()}>
            {t("ownerUsage.increase.mailButton", { email: SUPPORT_EMAIL })}
          </a>
          <Button variant="outline" onClick={() => void copy(draft)}>
            {copiedKey
              ? t("ownerUsage.increase.copied")
              : t("ownerUsage.increase.copyButton")}
          </Button>
        </div>
      </CardContent>
    </Card>
  )
}
