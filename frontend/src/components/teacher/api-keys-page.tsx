import { RelativeTime } from "@/components/relative-time"
import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query"
import { useTranslation } from "react-i18next"
import { apiKeysQuery } from "@/lib/queries"
import { api } from "@/lib/api"
import { useCopyFeedback } from "@/lib/use-copy-feedback"
import { Button } from "@/components/ui/button"
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card"
import { Input } from "@/components/ui/input"
import { ListRow, ListSkeleton, ListEmpty } from "@/components/ui/list"
import { ErrorText } from "@/components/ui/error-text"
import { useState } from "react"
import type { ApiKeyCreated } from "@/lib/types"

export function ApiKeysPage({ useParams }: { useParams: () => { courseId: string } }) {
  const { courseId } = useParams()
  const queryClient = useQueryClient()
  const { t } = useTranslation("teacher")
  const { t: tCommon } = useTranslation("common")
  const [keyName, setKeyName] = useState("")
  const [newKey, setNewKey] = useState<ApiKeyCreated | null>(null)
  const { copiedKey, copy } = useCopyFeedback()
  const { data: keys, isLoading } = useQuery(apiKeysQuery(courseId))

  const createMutation = useMutation({
    mutationFn: (data: { name: string }) =>
      api.post<ApiKeyCreated>(`/courses/${courseId}/api-keys`, data),
    onSuccess: (data) => {
      setNewKey(data)
      setKeyName("")
      queryClient.invalidateQueries({
        queryKey: apiKeysQuery(courseId).queryKey,
      })
    },
  })

  const deleteMutation = useMutation({
    mutationFn: (keyId: string) =>
      api.delete(`/courses/${courseId}/api-keys/${keyId}`),
    onSuccess: () => {
      queryClient.invalidateQueries({
        queryKey: apiKeysQuery(courseId).queryKey,
      })
    },
  })

  return (
    <Card>
      <CardHeader>
        <CardTitle>{t("apiKeys.title")}</CardTitle>
        <CardDescription>
          {t("apiKeys.description")}
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
            placeholder={t("apiKeys.namePlaceholder")}
            className="flex-1"
          />
          <Button type="submit" disabled={createMutation.isPending || !keyName.trim()}>
            {createMutation.isPending ? t("apiKeys.creatingButton") : t("apiKeys.createButton")}
          </Button>
        </form>

        {createMutation.isError && (
          <ErrorText error={createMutation.error} />
        )}

        {newKey && (
          <div className="rounded-md border border-amber-300 bg-amber-50 dark:bg-amber-950/20 dark:border-amber-800 p-4 space-y-2">
            <p className="text-sm font-medium">
              {t("apiKeys.createdBanner")}
            </p>
            <div className="flex gap-2 items-center">
              <code className="text-sm bg-muted px-3 py-2 rounded flex-1 font-mono break-all">
                {newKey.key}
              </code>
              <Button
                variant="outline"
                size="sm"
                onClick={() => void copy(newKey.key)}
              >
                {copiedKey ? t("apiKeys.copied") : tCommon("actions.copy")}
              </Button>
              <output className="sr-only">
                {copiedKey ? t("apiKeys.copied") : ""}
              </output>
            </div>
          </div>
        )}

        {isLoading && <ListSkeleton />}

        {keys && keys.length === 0 && !newKey && (
          <ListEmpty>{t("apiKeys.empty")}</ListEmpty>
        )}

        <div className="space-y-3">
          {keys?.map((k) => (
            <ListRow key={k.id}>
              <div className="space-y-1 flex-1 min-w-0">
                <div className="flex items-center gap-2">
                  <span className="font-medium text-sm">{k.name}</span>
                  <code className="text-xs bg-muted px-1.5 py-0.5 rounded">
                    {k.key_prefix}
                  </code>
                </div>
                <div className="flex gap-3 text-xs text-muted-foreground">
                  <span>{t("apiKeys.created")} <RelativeTime date={k.created_at} /></span>
                  {k.last_used_at && (
                    <span>{t("apiKeys.lastUsed")} <RelativeTime date={k.last_used_at} /></span>
                  )}
                </div>
              </div>
              <Button
                variant="destructive"
                size="sm"
                onClick={() => deleteMutation.mutate(k.id)}
                disabled={deleteMutation.isPending}
              >
                {t("apiKeys.revoke")}
              </Button>
            </ListRow>
          ))}
        </div>
      </CardContent>
    </Card>
  )
}
