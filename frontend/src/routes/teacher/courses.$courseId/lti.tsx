import { createFileRoute } from "@tanstack/react-router"
import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query"
import { useTranslation } from "react-i18next"
import { ltiSetupQuery, ltiRegistrationsQuery } from "@/lib/queries"
import { api } from "@/lib/api"
import { useApiErrorMessage } from "@/lib/use-api-error"
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
import { Badge } from "@/components/ui/badge"
import { Separator } from "@/components/ui/separator"
import { Skeleton } from "@/components/ui/skeleton"
import { useState } from "react"
import type { LtiRegistration } from "@/lib/types"

export const Route = createFileRoute("/teacher/courses/$courseId/lti")({
  component: LtiPage,
})

function LtiPage() {
  const { courseId } = Route.useParams()
  const queryClient = useQueryClient()
  const { t } = useTranslation("teacher")
  const { t: tCommon } = useTranslation("common")
  const formatError = useApiErrorMessage()
  const { data: setup } = useQuery(ltiSetupQuery(courseId))
  const { data: registrations, isLoading } = useQuery(ltiRegistrationsQuery(courseId))
  const [showForm, setShowForm] = useState(false)
  const [name, setName] = useState("")
  const [issuer, setIssuer] = useState("")
  const [clientId, setClientId] = useState("")
  const [copiedField, setCopiedField] = useState<string | null>(null)

  const createMutation = useMutation({
    mutationFn: (data: {
      name: string
      issuer: string
      client_id: string
    }) => api.post<LtiRegistration>(`/courses/${courseId}/lti`, data),
    onSuccess: () => {
      setShowForm(false)
      setName("")
      setIssuer("")
      setClientId("")
      queryClient.invalidateQueries({
        queryKey: ["courses", courseId, "lti"],
      })
    },
  })

  const deleteMutation = useMutation({
    mutationFn: (regId: string) =>
      api.delete(`/courses/${courseId}/lti/${regId}`),
    onSuccess: () => {
      queryClient.invalidateQueries({
        queryKey: ["courses", courseId, "lti"],
      })
    },
  })

  function copyToClipboard(text: string, field: string) {
    navigator.clipboard.writeText(text)
    setCopiedField(field)
    setTimeout(() => setCopiedField(null), 2000)
  }

  const config = setup?.moodle_tool_config

  return (
    <div className="space-y-4">
      <div className="rounded-md border border-amber-300 bg-amber-50 px-4 py-3 text-sm dark:border-amber-800 dark:bg-amber-950/40">
        <p className="font-semibold text-amber-900 dark:text-amber-200">{t("lti.noticeTitle")}</p>
        <ul className="mt-2 list-disc space-y-1 pl-5 text-amber-900/90 dark:text-amber-200/90">
          <li>{t("lti.noticeBullet1")}</li>
          <li>{t("lti.noticeBullet2")}</li>
          <li>{t("lti.noticeBullet3")}</li>
        </ul>
      </div>
      <Card>
        <CardHeader>
          <CardTitle>{t("lti.moodleConfigTitle")}</CardTitle>
          <CardDescription>
            {t("lti.moodleConfigDescription")}
          </CardDescription>
        </CardHeader>
        <CardContent className="space-y-3">
          {config ? (
            <>
              {[
                { label: t("lti.toolUrl"), value: config.tool_url, key: "tool_url" },
                { label: t("lti.ltiVersion"), value: config.lti_version, key: "lti_version" },
                { label: t("lti.publicKeyType"), value: config.public_key_type, key: "public_key_type" },
                { label: t("lti.publicKeysetUrl"), value: config.public_keyset_url, key: "keyset" },
                { label: t("lti.initiateLoginUrl"), value: config.initiate_login_url, key: "login" },
                { label: t("lti.redirectionUris"), value: config.redirection_uris, key: "redirect" },
                { label: t("lti.customParameters"), value: config.custom_parameters, key: "custom" },
                { label: t("lti.iconUrl"), value: config.icon_url, key: "icon" },
              ].map(({ label, value, key }) => (
                <div key={key} className="flex items-center justify-between gap-4">
                  <div className="min-w-0 flex-1">
                    <Label className="text-xs text-muted-foreground">{label}</Label>
                    <code className="block text-sm bg-muted px-2 py-1 rounded truncate">{value}</code>
                  </div>
                  <Button
                    variant="outline"
                    size="sm"
                    className="shrink-0"
                    onClick={() => copyToClipboard(value, key)}
                  >
                    {copiedField === key ? t("lti.copied") : tCommon("actions.copy")}
                  </Button>
                </div>
              ))}
              <Separator />
              <div className="text-sm text-muted-foreground space-y-1">
                <p>{t("lti.customParamExplainPrefix")}<strong>{t("lti.customParamExplainBoldCustom")}</strong>{t("lti.customParamExplainMid")}<code>user_eppn=$User.username</code>{t("lti.customParamExplainSuffix")}</p>
                <p>{t("lti.privacyNotePrefix")}<strong>{t("lti.privacyNoteBold")}</strong>{t("lti.privacyNoteSuffix")}</p>
              </div>
            </>
          ) : (
            <div className="space-y-2">
              <Skeleton className="h-8 w-full" />
              <Skeleton className="h-8 w-full" />
              <Skeleton className="h-8 w-full" />
            </div>
          )}
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle>{t("lti.registrationsTitle")}</CardTitle>
          <CardDescription>
            {t("lti.registrationsDescription")}
          </CardDescription>
        </CardHeader>
        <CardContent className="space-y-4">
          {!showForm && (
            <Button onClick={() => setShowForm(true)}>{t("lti.addMoodleConnection")}</Button>
          )}

          {showForm && (
            <form
              className="space-y-3 rounded-md border p-4"
              onSubmit={(e) => {
                e.preventDefault()
                createMutation.mutate({
                  name: name.trim(),
                  issuer: issuer.trim(),
                  client_id: clientId.trim(),
                })
              }}
            >
              <p className="text-sm text-muted-foreground">
                {t("lti.copyValuesHint")}
              </p>
              <div className="space-y-2">
                <Label htmlFor="lti-name">{t("lti.nameLabel")}</Label>
                <Input id="lti-name" value={name} onChange={(e) => setName(e.target.value)} placeholder={t("lti.namePlaceholder")} />
              </div>
              <div className="space-y-2">
                <Label htmlFor="lti-issuer">{t("lti.issuerLabel")}</Label>
                <Input id="lti-issuer" value={issuer} onChange={(e) => setIssuer(e.target.value)} placeholder={t("lti.issuerPlaceholder")} />
              </div>
              <div className="space-y-2">
                <Label htmlFor="lti-client-id">{t("lti.clientIdLabel")}</Label>
                <Input id="lti-client-id" value={clientId} onChange={(e) => setClientId(e.target.value)} />
              </div>

              {createMutation.isError && (
                <p className="text-sm text-destructive">{formatError(createMutation.error)}</p>
              )}

              <div className="flex gap-2">
                <Button type="submit" disabled={createMutation.isPending || !issuer.trim() || !clientId.trim()}>
                  {createMutation.isPending ? t("lti.saving") : t("lti.saveRegistration")}
                </Button>
                <Button type="button" variant="outline" onClick={() => setShowForm(false)}>
                  {tCommon("actions.cancel")}
                </Button>
              </div>
            </form>
          )}

          {isLoading && (
            <div className="space-y-2">
              <Skeleton className="h-10 w-full" />
            </div>
          )}

          {registrations && registrations.length === 0 && !showForm && (
            <p className="text-sm text-muted-foreground py-4 text-center">
              {t("lti.emptyRegistrations")}
            </p>
          )}

          <div className="space-y-3">
            {registrations?.map((reg) => (
              <div
                key={reg.id}
                className="flex items-center justify-between py-2 border-b last:border-0"
              >
                <div className="space-y-1 flex-1 min-w-0">
                  <div className="flex items-center gap-2">
                    <span className="font-medium text-sm">{reg.name}</span>
                    <Badge variant="secondary">{reg.client_id}</Badge>
                  </div>
                  <div className="text-xs text-muted-foreground truncate">{reg.issuer}</div>
                </div>
                <Button
                  variant="destructive"
                  size="sm"
                  onClick={() => deleteMutation.mutate(reg.id)}
                  disabled={deleteMutation.isPending}
                >
                  {t("lti.remove")}
                </Button>
              </div>
            ))}
          </div>
        </CardContent>
      </Card>
    </div>
  )
}
