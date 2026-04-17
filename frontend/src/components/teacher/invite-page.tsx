import { RelativeTime } from "@/components/relative-time"
import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query"
import { useTranslation } from "react-i18next"
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

export function InvitePage({ useParams }: { useParams: () => { courseId: string } }) {
  const { courseId } = useParams()
  const queryClient = useQueryClient()
  const { t } = useTranslation("teacher")
  const { t: tCommon } = useTranslation("common")
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
        <CardTitle>{t("invite.title")}</CardTitle>
        <CardDescription>
          {t("invite.description")}
        </CardDescription>
      </CardHeader>
      <CardContent className="space-y-4">
        <div className="flex gap-2 items-end">
          <div className="space-y-1">
            <Label className="text-xs">{t("invite.durationLabel")}</Label>
            <select
              value={expiresHours}
              onChange={(e) => setExpiresHours(Number(e.target.value))}
              className="border rounded px-2 py-1.5 text-sm bg-background"
            >
              <option value={1}>{t("invite.duration1h")}</option>
              <option value={24}>{t("invite.duration1d")}</option>
              <option value={168}>{t("invite.duration7d")}</option>
              <option value={720}>{t("invite.duration30d")}</option>
              <option value={8760}>{t("invite.duration1y")}</option>
            </select>
          </div>
          <div className="space-y-1">
            <Label className="text-xs">{t("invite.maxUsesLabel")}</Label>
            <Input
              type="number"
              value={maxUses}
              onChange={(e) => setMaxUses(e.target.value)}
              placeholder={t("invite.maxUsesPlaceholder")}
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
            {createMutation.isPending ? t("invite.generating") : t("invite.generate")}
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
                  <span>{t("invite.expires")} <RelativeTime date={link.expires_at} /></span>
                  <span>
                    {link.max_uses
                      ? t("invite.usedWithMax", { count: link.use_count, max: link.max_uses })
                      : t("invite.used", { count: link.use_count })}
                  </span>
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
                  {tCommon("actions.copy")}
                </Button>
                <Button
                  variant="ghost"
                  size="sm"
                  onClick={() => deleteMutation.mutate(link.id)}
                >
                  {t("invite.revoke")}
                </Button>
              </div>
            </div>
          ))}
        </div>
      </CardContent>
    </Card>
  )
}
