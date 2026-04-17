import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query"
import { useTranslation } from "react-i18next"
import {
  courseMembersQuery,
  courseQuery,
  courseRoleSuggestionsQuery,
} from "@/lib/queries"
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
import { Badge } from "@/components/ui/badge"
import { useState } from "react"

export function MembersPage({ useParams }: { useParams: () => { courseId: string } }) {
  const { courseId } = useParams()
  const { t } = useTranslation("teacher")
  const { t: tCommon } = useTranslation("common")
  const { data: members, isLoading } = useQuery(courseMembersQuery(courseId))
  const { data: course } = useQuery(courseQuery(courseId))
  const { data: suggestions } = useQuery(courseRoleSuggestionsQuery(courseId))
  const queryClient = useQueryClient()
  const [eppn, setEppn] = useState("")
  const [role, setRole] = useState("student")
  const canMutate = course?.my_role !== "ta"
  // Approve/decline is backend-gated to owner+admin; a course teacher who
  // isn't the owner still sees the suggestion list but the buttons would
  // 403. Hide them to avoid the confusing dead-click.
  const canResolveSuggestions = course?.my_role === "teacher"

  const invalidate = () => {
    queryClient.invalidateQueries({
      queryKey: ["courses", courseId, "members"],
    })
    queryClient.invalidateQueries({
      queryKey: ["courses", courseId, "role-suggestions"],
    })
  }

  const addMutation = useMutation({
    mutationFn: (data: { eppn: string; role: string }) =>
      api.post(`/courses/${courseId}/members`, data),
    onSuccess: () => {
      invalidate()
      setEppn("")
    },
  })

  const removeMutation = useMutation({
    mutationFn: (userId: string) =>
      api.delete(`/courses/${courseId}/members/${userId}`),
    onSuccess: invalidate,
  })

  const approveMutation = useMutation({
    mutationFn: (suggestionId: string) =>
      api.post(
        `/courses/${courseId}/role-suggestions/${suggestionId}/approve`,
        {},
      ),
    onSuccess: invalidate,
  })

  const declineMutation = useMutation({
    mutationFn: (suggestionId: string) =>
      api.post(
        `/courses/${courseId}/role-suggestions/${suggestionId}/decline`,
        {},
      ),
    onSuccess: invalidate,
  })

  return (
    <div className="space-y-4">
      {suggestions && suggestions.length > 0 && (
        <Card>
          <CardHeader>
            <CardTitle>
              {t("members.suggestionsTitle")}
              <Badge variant="secondary" className="ml-2">
                {suggestions.length}
              </Badge>
            </CardTitle>
            <CardDescription>
              {t("members.suggestionsDescription")}
            </CardDescription>
          </CardHeader>
          <CardContent className="space-y-2">
            {suggestions.map((s) => (
              <div
                key={s.id}
                className="flex flex-wrap items-center justify-between gap-2 py-2 border-b last:border-0"
              >
                <div className="min-w-0 break-words">
                  <div className="font-medium">
                    {s.display_name || s.eppn}
                  </div>
                  {s.display_name && (
                    <div className="text-muted-foreground text-sm break-all">
                      {s.eppn}
                    </div>
                  )}
                  <div className="text-xs text-muted-foreground mt-1">
                    {s.current_role ? (
                      <>
                        <Badge variant="outline" className="mr-1">
                          {s.current_role}
                        </Badge>
                        &rarr;
                        <Badge variant="default" className="ml-1">
                          {s.suggested_role}
                        </Badge>
                      </>
                    ) : (
                      <Badge variant="default">{s.suggested_role}</Badge>
                    )}
                    <span className="ml-2">{t("members.via", { source: s.source })}</span>
                    {s.source_detail?.lti_roles &&
                      s.source_detail.lti_roles.length > 0 && (
                        <span className="ml-2 break-all">
                          (
                          {s.source_detail.lti_roles
                            .map((r) => r.split("#").pop() ?? r)
                            .join(", ")}
                          )
                        </span>
                      )}
                  </div>
                </div>
                {canResolveSuggestions && (
                  <div className="flex gap-2 shrink-0">
                    <Button
                      size="sm"
                      onClick={() => approveMutation.mutate(s.id)}
                      disabled={approveMutation.isPending}
                    >
                      {t("members.approve")}
                    </Button>
                    <Button
                      size="sm"
                      variant="outline"
                      onClick={() => declineMutation.mutate(s.id)}
                      disabled={declineMutation.isPending}
                    >
                      {t("members.decline")}
                    </Button>
                  </div>
                )}
              </div>
            ))}
          </CardContent>
        </Card>
      )}

      <Card>
        <CardHeader>
          <CardTitle>{t("members.title")}</CardTitle>
          <CardDescription>
            {t("members.description")}
          </CardDescription>
        </CardHeader>
        <CardContent className="space-y-4">
          {canMutate && (
            <form
              className="flex flex-wrap gap-2"
              onSubmit={(e) => {
                e.preventDefault()
                if (eppn) addMutation.mutate({ eppn, role })
              }}
            >
              <Input
                value={eppn}
                onChange={(e) => setEppn(e.target.value)}
                placeholder={t("members.eppnPlaceholder")}
                className="flex-1 min-w-[12rem]"
              />
              <select
                value={role}
                onChange={(e) => setRole(e.target.value)}
                className="border rounded px-2 py-1 text-sm bg-background"
              >
                <option value="student">{t("members.roleStudent")}</option>
                <option value="ta">{t("members.roleTA")}</option>
                <option value="teacher">{t("members.roleTeacher")}</option>
              </select>
              <Button type="submit" disabled={addMutation.isPending}>
                {t("members.add")}
              </Button>
            </form>
          )}

          {isLoading && <p className="text-muted-foreground">{tCommon("status.loading")}</p>}

          <div className="space-y-2">
            {members?.map((m) => (
              <div
                key={m.user_id}
                className="flex flex-wrap items-center justify-between gap-2 py-2 border-b last:border-0"
              >
                <div className="min-w-0 break-words">
                  <span className="font-medium">
                    {m.display_name || m.eppn}
                  </span>
                  {m.display_name && (
                    <span className="text-muted-foreground text-sm ml-2 break-all">
                      {m.eppn}
                    </span>
                  )}
                </div>
                <div className="flex items-center gap-2 shrink-0">
                  <Badge variant="outline">{m.role}</Badge>
                  {canMutate && (
                    <Button
                      variant="ghost"
                      size="sm"
                      onClick={() => removeMutation.mutate(m.user_id)}
                    >
                      {t("members.remove")}
                    </Button>
                  )}
                </div>
              </div>
            ))}
          </div>
        </CardContent>
      </Card>
    </div>
  )
}
