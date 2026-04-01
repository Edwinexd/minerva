import { createFileRoute } from "@tanstack/react-router"
import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query"
import { api } from "@/lib/api"
import { Button } from "@/components/ui/button"
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"
import { Skeleton } from "@/components/ui/skeleton"
import { useState } from "react"

export const Route = createFileRoute("/teacher/courses/$courseId/invite")({
  component: InvitePage,
})

function InvitePage() {
  const { courseId } = Route.useParams()
  const queryClient = useQueryClient()
  const [expiresHours, setExpiresHours] = useState(168)
  const [maxUses, setMaxUses] = useState("")
  const { data: links, isLoading } = useQuery({
    queryKey: ["courses", courseId, "signed-urls"],
    queryFn: () =>
      api.get<
        {
          id: string
          token: string
          url: string
          expires_at: string
          max_uses: number | null
          use_count: number
        }[]
      >(`/courses/${courseId}/signed-urls`),
  })

  const createMutation = useMutation({
    mutationFn: (data: { expires_in_hours?: number; max_uses?: number }) =>
      api.post(`/courses/${courseId}/signed-urls`, data),
    onSuccess: () => {
      queryClient.invalidateQueries({
        queryKey: ["courses", courseId, "signed-urls"],
      })
    },
  })

  const deleteMutation = useMutation({
    mutationFn: (id: string) =>
      api.delete(`/courses/${courseId}/signed-urls/${id}`),
    onSuccess: () => {
      queryClient.invalidateQueries({
        queryKey: ["courses", courseId, "signed-urls"],
      })
    },
  })

  return (
    <Card>
      <CardHeader>
        <CardTitle>Invite Links</CardTitle>
        <CardDescription>
          Generate signed URLs for students to join this course
        </CardDescription>
      </CardHeader>
      <CardContent className="space-y-4">
        <div className="flex gap-2 items-end">
          <div className="space-y-1">
            <Label className="text-xs">Duration</Label>
            <select
              value={expiresHours}
              onChange={(e) => setExpiresHours(Number(e.target.value))}
              className="border rounded px-2 py-1.5 text-sm bg-background"
            >
              <option value={1}>1 hour</option>
              <option value={24}>1 day</option>
              <option value={168}>7 days</option>
              <option value={720}>30 days</option>
              <option value={8760}>1 year</option>
            </select>
          </div>
          <div className="space-y-1">
            <Label className="text-xs">Max uses (optional)</Label>
            <Input
              type="number"
              value={maxUses}
              onChange={(e) => setMaxUses(e.target.value)}
              placeholder="unlimited"
              className="w-28"
              min={1}
            />
          </div>
          <Button
            onClick={() => createMutation.mutate({
              expires_in_hours: expiresHours,
              max_uses: maxUses ? parseInt(maxUses) : undefined,
            })}
            disabled={createMutation.isPending}
          >
            {createMutation.isPending ? "Generating..." : "Generate Link"}
          </Button>
        </div>

        {isLoading && (
          <div className="space-y-2">
            <Skeleton className="h-10 w-full" />
            <Skeleton className="h-10 w-full" />
          </div>
        )}

        <div className="space-y-3">
          {links?.map((link) => (
            <div
              key={link.id}
              className="flex items-center justify-between py-2 border-b last:border-0"
            >
              <div className="space-y-1 flex-1 min-w-0">
                <code className="text-xs bg-muted px-2 py-1 rounded block truncate">
                  {window.location.origin}/join/{link.token}
                </code>
                <div className="flex gap-3 text-xs text-muted-foreground">
                  <span>Expires: {new Date(link.expires_at).toLocaleDateString()}</span>
                  <span>Used: {link.use_count}{link.max_uses ? `/${link.max_uses}` : ""}</span>
                </div>
              </div>
              <div className="flex gap-2 ml-2">
                <Button
                  variant="outline"
                  size="sm"
                  onClick={() => {
                    navigator.clipboard.writeText(
                      `${window.location.origin}/join/${link.token}`,
                    )
                  }}
                >
                  Copy
                </Button>
                <Button
                  variant="ghost"
                  size="sm"
                  onClick={() => deleteMutation.mutate(link.id)}
                >
                  Revoke
                </Button>
              </div>
            </div>
          ))}
        </div>
      </CardContent>
    </Card>
  )
}
