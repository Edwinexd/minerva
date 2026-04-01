import { createFileRoute } from "@tanstack/react-router"
import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query"
import { apiKeysQuery } from "@/lib/queries"
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
import { Skeleton } from "@/components/ui/skeleton"
import { useState } from "react"
import type { ApiKeyCreated } from "@/lib/types"

export const Route = createFileRoute("/teacher/courses/$courseId/api-keys")({
  component: ApiKeysPage,
})

function ApiKeysPage() {
  const { courseId } = Route.useParams()
  const queryClient = useQueryClient()
  const [keyName, setKeyName] = useState("")
  const [newKey, setNewKey] = useState<ApiKeyCreated | null>(null)
  const [copied, setCopied] = useState(false)
  const { data: keys, isLoading } = useQuery(apiKeysQuery(courseId))

  const createMutation = useMutation({
    mutationFn: (data: { name: string }) =>
      api.post<ApiKeyCreated>(`/courses/${courseId}/api-keys`, data),
    onSuccess: (data) => {
      setNewKey(data)
      setKeyName("")
      queryClient.invalidateQueries({
        queryKey: ["courses", courseId, "api-keys"],
      })
    },
  })

  const deleteMutation = useMutation({
    mutationFn: (keyId: string) =>
      api.delete(`/courses/${courseId}/api-keys/${keyId}`),
    onSuccess: () => {
      queryClient.invalidateQueries({
        queryKey: ["courses", courseId, "api-keys"],
      })
    },
  })

  return (
    <Card>
      <CardHeader>
        <CardTitle>API Keys</CardTitle>
        <CardDescription>
          Create API keys for external integrations (e.g. Moodle plugin).
          Keys are scoped to this course only.
        </CardDescription>
      </CardHeader>
      <CardContent className="space-y-4">
        <form
          className="flex gap-2"
          onSubmit={(e) => {
            e.preventDefault()
            if (keyName.trim()) {
              setNewKey(null)
              createMutation.mutate({ name: keyName.trim() })
            }
          }}
        >
          <Input
            value={keyName}
            onChange={(e) => setKeyName(e.target.value)}
            placeholder="Key name (e.g. Moodle integration)"
            className="flex-1"
          />
          <Button type="submit" disabled={createMutation.isPending || !keyName.trim()}>
            {createMutation.isPending ? "Creating..." : "Create Key"}
          </Button>
        </form>

        {createMutation.isError && (
          <p className="text-sm text-destructive">{createMutation.error.message}</p>
        )}

        {newKey && (
          <div className="rounded-md border border-amber-300 bg-amber-50 dark:bg-amber-950/20 dark:border-amber-800 p-4 space-y-2">
            <p className="text-sm font-medium">
              API key created! Copy it now - it won't be shown again.
            </p>
            <div className="flex gap-2 items-center">
              <code className="text-sm bg-muted px-3 py-2 rounded flex-1 font-mono break-all">
                {newKey.key}
              </code>
              <Button
                variant="outline"
                size="sm"
                onClick={() => {
                  navigator.clipboard.writeText(newKey.key)
                  setCopied(true)
                  setTimeout(() => setCopied(false), 2000)
                }}
              >
                {copied ? "Copied!" : "Copy"}
              </Button>
            </div>
          </div>
        )}

        {isLoading && (
          <div className="space-y-2">
            <Skeleton className="h-10 w-full" />
            <Skeleton className="h-10 w-full" />
          </div>
        )}

        {keys && keys.length === 0 && !newKey && (
          <p className="text-sm text-muted-foreground py-4 text-center">
            No API keys yet. Create one to integrate with external services.
          </p>
        )}

        <div className="space-y-3">
          {keys?.map((k) => (
            <div
              key={k.id}
              className="flex items-center justify-between py-2 border-b last:border-0"
            >
              <div className="space-y-1 flex-1 min-w-0">
                <div className="flex items-center gap-2">
                  <span className="font-medium text-sm">{k.name}</span>
                  <code className="text-xs bg-muted px-1.5 py-0.5 rounded">
                    {k.key_prefix}
                  </code>
                </div>
                <div className="flex gap-3 text-xs text-muted-foreground">
                  <span>Created: {new Date(k.created_at).toLocaleDateString()}</span>
                  {k.last_used_at && (
                    <span>Last used: {new Date(k.last_used_at).toLocaleDateString()}</span>
                  )}
                </div>
              </div>
              <Button
                variant="destructive"
                size="sm"
                onClick={() => deleteMutation.mutate(k.id)}
                disabled={deleteMutation.isPending}
              >
                Revoke
              </Button>
            </div>
          ))}
        </div>
      </CardContent>
    </Card>
  )
}
