import { createFileRoute } from "@tanstack/react-router"
import { useQuery } from "@tanstack/react-query"
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

export const Route = createFileRoute("/teacher/courses/$courseId/usage")({
  component: UsagePage,
})

interface UsageRow {
  user_id: string
  course_id: string
  date: string
  prompt_tokens: number
  completion_tokens: number
  embedding_tokens: number
  request_count: number
}

function formatTokens(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`
  if (n >= 1_000) return `${(n / 1_000).toFixed(1)}k`
  return n.toString()
}

function UsagePage() {
  const { courseId } = Route.useParams()
  const { data: usage, isLoading } = useQuery({
    queryKey: ["courses", courseId, "usage"],
    queryFn: () => api.get<UsageRow[]>(`/courses/${courseId}/usage`),
  })
  const { data: members } = useQuery(courseMembersQuery(courseId))

  const userMap = new Map<string, string>()
  for (const m of members || []) {
    userMap.set(m.user_id, m.display_name || m.eppn || m.user_id)
  }

  const byUser = new Map<string, { prompt: number; completion: number; embedding: number; requests: number }>()
  for (const row of usage || []) {
    const existing = byUser.get(row.user_id) || { prompt: 0, completion: 0, embedding: 0, requests: 0 }
    existing.prompt += row.prompt_tokens
    existing.completion += row.completion_tokens
    existing.embedding += row.embedding_tokens
    existing.requests += row.request_count
    byUser.set(row.user_id, existing)
  }

  let totalPrompt = 0
  let totalCompletion = 0
  let totalEmbedding = 0
  let totalRequests = 0
  for (const v of byUser.values()) {
    totalPrompt += v.prompt
    totalCompletion += v.completion
    totalEmbedding += v.embedding
    totalRequests += v.requests
  }

  return (
    <Card>
      <CardHeader>
        <CardTitle>Token Usage</CardTitle>
        <CardDescription>
          Track token consumption per student for billing and monitoring
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
          <p className="text-muted-foreground text-sm">No usage data yet.</p>
        )}

        {byUser.size > 0 && (
          <>
            <div className="grid grid-cols-4 gap-4 text-center">
              <div>
                <p className="text-2xl font-bold">{formatTokens(totalPrompt + totalCompletion)}</p>
                <p className="text-xs text-muted-foreground">Total tokens</p>
              </div>
              <div>
                <p className="text-2xl font-bold">{totalRequests}</p>
                <p className="text-xs text-muted-foreground">Requests</p>
              </div>
              <div>
                <p className="text-2xl font-bold">{formatTokens(totalPrompt)}</p>
                <p className="text-xs text-muted-foreground">Prompt tokens</p>
              </div>
              <div>
                <p className="text-2xl font-bold">{formatTokens(totalCompletion)}</p>
                <p className="text-xs text-muted-foreground">Completion tokens</p>
              </div>
            </div>

            <Separator />

            <div className="space-y-1">
              <div className="grid grid-cols-5 gap-2 text-xs font-medium text-muted-foreground px-2 pb-1">
                <span>User</span>
                <span className="text-right">Prompt</span>
                <span className="text-right">Completion</span>
                <span className="text-right">Total</span>
                <span className="text-right">Requests</span>
              </div>
              {Array.from(byUser.entries())
                .sort((a, b) => (b[1].prompt + b[1].completion) - (a[1].prompt + a[1].completion))
                .map(([userId, stats]) => (
                  <div key={userId} className="grid grid-cols-5 gap-2 text-sm px-2 py-1.5 border-b last:border-0">
                    <span className="truncate">{userMap.get(userId) || userId.slice(0, 8)}</span>
                    <span className="text-right text-muted-foreground">{formatTokens(stats.prompt)}</span>
                    <span className="text-right text-muted-foreground">{formatTokens(stats.completion)}</span>
                    <span className="text-right font-medium">{formatTokens(stats.prompt + stats.completion)}</span>
                    <span className="text-right text-muted-foreground">{stats.requests}</span>
                  </div>
                ))}
            </div>
          </>
        )}

        {usage && usage.length > 0 && (
          <>
            <Separator />
            <div>
              <h4 className="text-sm font-medium mb-2">Daily breakdown</h4>
              <div className="space-y-1 max-h-64 overflow-y-auto">
                <div className="grid grid-cols-5 gap-2 text-xs font-medium text-muted-foreground px-2 pb-1">
                  <span>Date</span>
                  <span>User</span>
                  <span className="text-right">Prompt</span>
                  <span className="text-right">Completion</span>
                  <span className="text-right">Requests</span>
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
