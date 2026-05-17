import { useQuery } from "@tanstack/react-query"
import { useTranslation } from "react-i18next"
import { courseMembersQuery } from "@/lib/queries"
import { api } from "@/lib/api"
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card"
import { Separator } from "@/components/ui/separator"
import { Skeleton } from "@/components/ui/skeleton"

interface UsageRow {
  user_id: string
  course_id: string
  date: string
  prompt_tokens: number
  completion_tokens: number
  embedding_tokens: number
  /**
   * Research-phase prompt-token share of `prompt_tokens` for the
   * day; lets us nest "research / writeup" under the prompt total
   * instead of treating research as a flat sibling axis. Zero on
   * days without `tool_use_enabled` traffic.
   */
  research_prompt_tokens: number
  /** Research-phase completion-token share of `completion_tokens`. */
  research_completion_tokens: number
  request_count: number
}

function formatTokens(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`
  if (n >= 1_000) return `${(n / 1_000).toFixed(1)}k`
  return n.toString()
}

export function UsagePage({ useParams }: { useParams: () => { courseId: string } }) {
  const { courseId } = useParams()
  const { t } = useTranslation("teacher")
  const { data: usage, isLoading } = useQuery({
    queryKey: ["courses", courseId, "usage"],
    queryFn: () => api.get<UsageRow[]>(`/courses/${courseId}/usage`),
  })
  const { data: members } = useQuery(courseMembersQuery(courseId))

  const userMap = new Map<string, string>()
  for (const m of members || []) {
    userMap.set(m.user_id, m.display_name || m.eppn || m.user_id)
  }

  const byUser = new Map<
    string,
    {
      prompt: number
      completion: number
      embedding: number
      researchPrompt: number
      researchCompletion: number
      requests: number
    }
  >()
  for (const row of usage || []) {
    const existing = byUser.get(row.user_id) || {
      prompt: 0,
      completion: 0,
      embedding: 0,
      researchPrompt: 0,
      researchCompletion: 0,
      requests: 0,
    }
    existing.prompt += row.prompt_tokens
    existing.completion += row.completion_tokens
    existing.embedding += row.embedding_tokens
    existing.researchPrompt += row.research_prompt_tokens
    existing.researchCompletion += row.research_completion_tokens
    existing.requests += row.request_count
    byUser.set(row.user_id, existing)
  }

  let totalPrompt = 0
  let totalCompletion = 0
  let totalResearchPrompt = 0
  let totalResearchCompletion = 0
  let totalRequests = 0
  for (const v of byUser.values()) {
    totalPrompt += v.prompt
    totalCompletion += v.completion
    totalResearchPrompt += v.researchPrompt
    totalResearchCompletion += v.researchCompletion
    totalRequests += v.requests
  }
  // Writeup subsets are derived: anything that isn't research is
  // writeup. Keeps research / writeup nested *inside* the prompt and
  // completion totals rather than promoted to flat siblings of them.
  const writeupPrompt = totalPrompt - totalResearchPrompt
  const writeupCompletion = totalCompletion - totalResearchCompletion
  const hasResearch = totalResearchPrompt + totalResearchCompletion > 0

  return (
    <Card>
      <CardHeader>
        <CardTitle>{t("usage.title")}</CardTitle>
        <CardDescription>
          {t("usage.description")}
        </CardDescription>
      </CardHeader>
      <CardContent className="space-y-4">
        {isLoading && (
          <div className="space-y-2">
            <Skeleton className="h-10 w-full" />
            <Skeleton className="h-10 w-full" />
          </div>
        )}

        {!isLoading && byUser.size === 0 && (
          <p className="text-muted-foreground text-sm">{t("usage.empty")}</p>
        )}

        {byUser.size > 0 && (
          <>
            <div className="grid grid-cols-2 gap-4 text-center sm:grid-cols-4">
              <div>
                <p className="text-2xl font-bold">{formatTokens(totalPrompt + totalCompletion)}</p>
                <p className="text-xs text-muted-foreground">{t("usage.totalTokens")}</p>
              </div>
              <div>
                <p className="text-2xl font-bold">{totalRequests}</p>
                <p className="text-xs text-muted-foreground">{t("usage.requests")}</p>
              </div>
              <div>
                <p className="text-2xl font-bold">{formatTokens(totalPrompt)}</p>
                <p className="text-xs text-muted-foreground">{t("usage.promptTokens")}</p>
                {hasResearch && (
                  <p className="text-[11px] text-muted-foreground/80 mt-0.5">
                    {t("usage.subsetBreakdown", {
                      research: formatTokens(totalResearchPrompt),
                      writeup: formatTokens(writeupPrompt),
                    })}
                  </p>
                )}
              </div>
              <div>
                <p className="text-2xl font-bold">{formatTokens(totalCompletion)}</p>
                <p className="text-xs text-muted-foreground">{t("usage.completionTokens")}</p>
                {hasResearch && (
                  <p className="text-[11px] text-muted-foreground/80 mt-0.5">
                    {t("usage.subsetBreakdown", {
                      research: formatTokens(totalResearchCompletion),
                      writeup: formatTokens(writeupCompletion),
                    })}
                  </p>
                )}
              </div>
            </div>

            <Separator />

            <div className="space-y-1">
              <div className="grid grid-cols-5 gap-2 text-xs font-medium text-muted-foreground px-2 pb-1">
                <span>{t("usage.colUser")}</span>
                <span className="text-right">{t("usage.colPrompt")}</span>
                <span className="text-right">{t("usage.colCompletion")}</span>
                <span className="text-right">{t("usage.colTotal")}</span>
                <span className="text-right">{t("usage.colRequests")}</span>
              </div>
              {Array.from(byUser.entries())
                .sort((a, b) => (b[1].prompt + b[1].completion) - (a[1].prompt + a[1].completion))
                .map(([userId, stats]) => {
                  const total = stats.prompt + stats.completion
                  const rowResearch = stats.researchPrompt + stats.researchCompletion
                  return (
                    <div key={userId} className="grid grid-cols-5 gap-2 text-sm px-2 py-1.5 border-b last:border-0">
                      <span className="truncate">{userMap.get(userId) || userId.slice(0, 8)}</span>
                      <span className="text-right text-muted-foreground">{formatTokens(stats.prompt)}</span>
                      <span className="text-right text-muted-foreground">{formatTokens(stats.completion)}</span>
                      <span className="text-right font-medium">
                        {formatTokens(total)}
                        {rowResearch > 0 && (
                          <span className="block text-[10px] font-normal text-muted-foreground/80">
                            {t("usage.subsetBreakdown", {
                              research: formatTokens(rowResearch),
                              writeup: formatTokens(total - rowResearch),
                            })}
                          </span>
                        )}
                      </span>
                      <span className="text-right text-muted-foreground">{stats.requests}</span>
                    </div>
                  )
                })}
            </div>
          </>
        )}

        {usage && usage.length > 0 && (
          <>
            <Separator />
            <div>
              <h4 className="text-sm font-medium mb-2">{t("usage.dailyBreakdown")}</h4>
              <div className="space-y-1 max-h-64 overflow-y-auto">
                <div className="grid grid-cols-5 gap-2 text-xs font-medium text-muted-foreground px-2 pb-1">
                  <span>{t("usage.colDate")}</span>
                  <span>{t("usage.colUser")}</span>
                  <span className="text-right">{t("usage.colPrompt")}</span>
                  <span className="text-right">{t("usage.colCompletion")}</span>
                  <span className="text-right">{t("usage.colRequests")}</span>
                </div>
                {usage.map((row, i) => (
                  <div key={i} className="grid grid-cols-5 gap-2 text-xs px-2 py-1 border-b last:border-0">
                    <span>{row.date}</span>
                    <span className="truncate">{userMap.get(row.user_id) || row.user_id.slice(0, 8)}</span>
                    <span className="text-right">{formatTokens(row.prompt_tokens)}</span>
                    <span className="text-right">{formatTokens(row.completion_tokens)}</span>
                    <span className="text-right">{row.request_count}</span>
                  </div>
                ))}
              </div>
            </div>
          </>
        )}
      </CardContent>
    </Card>
  )
}
