import { useMemo, useState } from "react"
import { useTranslation } from "react-i18next"
import { useMutation, useQuery } from "@tanstack/react-query"
import { api } from "@/lib/api"
import { useApiErrorMessage } from "@/lib/use-api-error"
import type { LtiBindInfo } from "@/lib/types"
import { Button } from "@/components/ui/button"
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card"
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select"
import { Skeleton } from "@/components/ui/skeleton"

/// First-launch LTI bind picker. Reachable without Shibboleth -- the token
/// in the URL is the auth. Renders when the backend's launch handler
/// couldn't find an (platform, context) → course binding and redirected
/// the user here to pick one.
export function LtiBindPage() {
  const { t } = useTranslation("auth")
  const formatError = useApiErrorMessage()

  const token = useMemo(() => {
    const params = new URLSearchParams(window.location.search)
    return params.get("token") ?? ""
  }, [])

  const [selectedCourseId, setSelectedCourseId] = useState<string>("")

  const { data, isLoading, error } = useQuery({
    queryKey: ["lti", "bind", token],
    queryFn: () =>
      api.get<LtiBindInfo>(`/lti/bind?token=${encodeURIComponent(token)}`),
    enabled: token.length > 0,
    retry: false,
  })

  const mutation = useMutation({
    mutationFn: async () => {
      const res = await api.post<{ redirect_url: string }>("/lti/bind", {
        token,
        course_id: selectedCourseId,
      })
      return res
    },
    onSuccess: (res) => {
      window.location.replace(res.redirect_url)
    },
  })

  if (!token) {
    return (
      <Card className="max-w-xl mx-auto">
        <CardHeader>
          <CardTitle>{t("ltiBind.errorTitle")}</CardTitle>
          <CardDescription>{t("ltiBind.missingToken")}</CardDescription>
        </CardHeader>
      </Card>
    )
  }

  if (isLoading) {
    return (
      <div className="max-w-xl mx-auto space-y-3">
        <Skeleton className="h-20 w-full" />
        <Skeleton className="h-40 w-full" />
      </div>
    )
  }

  if (error || !data) {
    return (
      <Card className="max-w-xl mx-auto">
        <CardHeader>
          <CardTitle>{t("ltiBind.errorTitle")}</CardTitle>
          <CardDescription>
            {error ? formatError(error) : t("ltiBind.loadFailed")}
          </CardDescription>
        </CardHeader>
      </Card>
    )
  }

  const contextDisplay =
    data.context_title || data.context_label || data.context_id

  // Non-teachers can't bind. Show a friendly "ask your teacher" state
  // instead of an empty dropdown.
  if (data.courses.length === 0) {
    return (
      <Card className="max-w-xl mx-auto">
        <CardHeader>
          <CardTitle>{t("ltiBind.notTeacherTitle")}</CardTitle>
          <CardDescription>
            {t("ltiBind.notTeacherBody", {
              platform: data.platform_name,
              context: contextDisplay,
            })}
          </CardDescription>
        </CardHeader>
      </Card>
    )
  }

  return (
    <Card className="max-w-xl mx-auto">
      <CardHeader>
        <CardTitle>{t("ltiBind.title")}</CardTitle>
        <CardDescription>
          {t("ltiBind.description", {
            platform: data.platform_name,
            context: contextDisplay,
          })}
        </CardDescription>
      </CardHeader>
      <CardContent className="space-y-4">
        {!data.is_teacher_role && (
          <div className="rounded-md border border-amber-300 bg-amber-50 px-3 py-2 text-sm dark:border-amber-700 dark:bg-amber-950/40">
            {t("ltiBind.nonTeacherRoleWarning")}
          </div>
        )}

        <div className="space-y-2">
          <label className="text-sm font-medium">{t("ltiBind.courseLabel")}</label>
          <Select
            value={selectedCourseId}
            onValueChange={(v) => setSelectedCourseId(v ?? "")}
          >
            <SelectTrigger>
              <SelectValue placeholder={t("ltiBind.coursePlaceholder")} />
            </SelectTrigger>
            <SelectContent>
              {data.courses.map((c) => (
                <SelectItem key={c.id} value={c.id}>
                  {c.name}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </div>

        <p className="text-xs text-muted-foreground">{t("ltiBind.linkNote")}</p>

        {mutation.isError && (
          <p className="text-sm text-destructive">{formatError(mutation.error)}</p>
        )}

        <Button
          onClick={() => mutation.mutate()}
          disabled={!selectedCourseId || mutation.isPending}
          className="w-full"
        >
          {mutation.isPending ? t("ltiBind.linking") : t("ltiBind.linkButton")}
        </Button>
      </CardContent>
    </Card>
  )
}
