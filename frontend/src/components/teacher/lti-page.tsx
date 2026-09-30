import {
  useInfiniteQuery,
  useQuery,
  useMutation,
  useQueryClient,
} from "@tanstack/react-query"
import { useTranslation } from "react-i18next"
import {
  ltiCourseSiteBindingsQuery,
  ltiNrpsOlderRunsQuery,
  ltiNrpsStatusQuery,
  ltiRegistrationsQuery,
  ltiSetupQuery,
} from "@/lib/queries"
import { api } from "@/lib/api"
import { LtiManualConfigTable } from "@/components/lti-manual-config-table"
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
import { Checkbox } from "@/components/ui/checkbox"
import { Separator } from "@/components/ui/separator"
import { Skeleton } from "@/components/ui/skeleton"
import { ListRow, ListEmpty } from "@/components/ui/list"
import { ErrorText } from "@/components/ui/error-text"
import { RelativeTime } from "@/components/relative-time"
import { useState } from "react"
import type { LtiNrpsStatus, LtiRegistration } from "@/lib/types"

export function LtiPage({ useParams }: { useParams: () => { courseId: string } }) {
  const { courseId } = useParams()
  const queryClient = useQueryClient()
  const { t } = useTranslation("teacher")
  const { t: tCommon } = useTranslation("common")
  const { data: setup } = useQuery(ltiSetupQuery(courseId))
  const { data: registrations, isLoading } = useQuery(ltiRegistrationsQuery(courseId))
  const { data: nrps } = useQuery(ltiNrpsStatusQuery(courseId))
  const { data: siteBindings } = useQuery(ltiCourseSiteBindingsQuery(courseId))
  const [showForm, setShowForm] = useState(false)
  const [name, setName] = useState("")
  const [issuer, setIssuer] = useState("")
  const [clientId, setClientId] = useState("")
  // Prefix key, so this also covers the nrps / site-bindings caches.
  const invalidateLti = () =>
    queryClient.invalidateQueries({
      queryKey: ltiRegistrationsQuery(courseId).queryKey,
    })

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
      invalidateLti()
    },
  })

  const deleteMutation = useMutation({
    mutationFn: (regId: string) =>
      api.delete(`/courses/${courseId}/lti/${regId}`),
    onSuccess: invalidateLti,
  })

  const unlinkSiteBindingMutation = useMutation({
    mutationFn: (bindingId: string) =>
      api.delete(`/courses/${courseId}/lti/site-bindings/${bindingId}`),
    onSuccess: invalidateLti,
  })

  const nrpsSyncMutation = useMutation({
    mutationFn: ({ id, enabled }: { id: string; enabled: boolean }) =>
      api.put(`/courses/${courseId}/lti/nrps/${id}/sync-enabled`, { enabled }),
    onSuccess: invalidateLti,
  })

  const config = setup?.moodle_tool_config

  // A site-level link shows its own roster sync; what is left over (the
  // per-course registrations' contexts) gets the standalone card.
  const siteNrps = new Map<string, LtiNrpsStatus>()
  const otherNrps: LtiNrpsStatus[] = []
  for (const ctx of nrps ?? []) {
    const linked =
      ctx.source === "platform" &&
      !siteNrps.has(ctx.context_id) &&
      siteBindings?.some((b) => b.context_id === ctx.context_id)
    if (linked) {
      siteNrps.set(ctx.context_id, ctx)
    } else {
      otherNrps.push(ctx)
    }
  }

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
              <LtiManualConfigTable config={config} ns="teacher" keyPrefix="lti" />
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
                <ErrorText error={createMutation.error} />
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
            <ListEmpty>{t("lti.emptyRegistrations")}</ListEmpty>
          )}

          <div className="space-y-3">
            {registrations?.map((reg) => (
              <ListRow key={reg.id}>
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
              </ListRow>
            ))}
          </div>
        </CardContent>
      </Card>

      {siteBindings && siteBindings.length > 0 && (
        <Card>
          <CardHeader>
            <CardTitle>{t("lti.siteBindingsTitle")}</CardTitle>
            <CardDescription>{t("lti.siteBindingsDescription")}</CardDescription>
          </CardHeader>
          <CardContent className="space-y-3">
            {siteBindings.map((b) => {
              const contextLabel =
                b.context_title || b.context_label || b.context_id
              const ctx = siteNrps.get(b.context_id)
              return (
                <div
                  key={b.id}
                  className="space-y-3 border-b py-2 text-sm last:border-0"
                >
                  <div className="flex flex-wrap items-center justify-between gap-2">
                    <div className="min-w-0 flex-1 space-y-1">
                      <div className="flex flex-wrap items-center gap-2">
                        <span className="font-medium">{contextLabel}</span>
                        <Badge variant="secondary">{b.platform_name}</Badge>
                      </div>
                      <div className="text-xs text-muted-foreground break-all">
                        {b.platform_issuer}
                      </div>
                      <div className="text-xs text-muted-foreground break-all">
                        {t("lti.siteBindingsContextIdLabel")}: {b.context_id}
                      </div>
                    </div>
                    <Button
                      variant="outline"
                      size="sm"
                      onClick={() => {
                        if (window.confirm(t("lti.siteBindingsUnlinkConfirm"))) {
                          unlinkSiteBindingMutation.mutate(b.id)
                        }
                      }}
                      disabled={unlinkSiteBindingMutation.isPending}
                    >
                      {unlinkSiteBindingMutation.isPending
                        ? t("lti.siteBindingsUnlinking")
                        : t("lti.siteBindingsUnlink")}
                    </Button>
                  </div>
                  {ctx && (
                    <NrpsSync
                      courseId={courseId}
                      ctx={ctx}
                      heading={t("lti.nrpsTitle")}
                      label={t("lti.nrpsSyncLabelPlain")}
                      pending={nrpsSyncMutation.isPending}
                      onToggle={(enabled) =>
                        nrpsSyncMutation.mutate({ id: ctx.id, enabled })
                      }
                    />
                  )}
                </div>
              )
            })}
            {unlinkSiteBindingMutation.isError && (
              <ErrorText error={unlinkSiteBindingMutation.error} />
            )}
          </CardContent>
        </Card>
      )}

      {otherNrps.length > 0 && (
        <Card>
          <CardHeader>
            <CardTitle>{t("lti.nrpsTitle")}</CardTitle>
            <CardDescription>{t("lti.nrpsDescription")}</CardDescription>
          </CardHeader>
          <CardContent className="space-y-3">
            {otherNrps.map((ctx) => (
              <div
                key={ctx.id}
                className="space-y-2 border-b py-2 text-sm last:border-0"
              >
                <NrpsSync
                  courseId={courseId}
                  ctx={ctx}
                  label={t("lti.nrpsSyncLabel", { context: ctx.context_id })}
                  pending={nrpsSyncMutation.isPending}
                  onToggle={(enabled) =>
                    nrpsSyncMutation.mutate({ id: ctx.id, enabled })
                  }
                />
              </div>
            ))}
          </CardContent>
        </Card>
      )}
      {nrpsSyncMutation.isError && (
        <ErrorText error={nrpsSyncMutation.error} />
      )}
    </div>
  )
}

/// Runs listed when the history is first opened, and how many each "Show
/// older" adds (the server's page size).
const NRPS_HISTORY_PREVIEW = 5
const NRPS_HISTORY_PAGE = 20

/// One LMS context's roster sync: the on/off switch, the last run and the
/// recorded history.
function NrpsSync({
  courseId,
  ctx,
  heading,
  label,
  pending,
  onToggle,
}: {
  courseId: string
  ctx: LtiNrpsStatus
  /// Given when the block sits inside another row: it is then boxed under
  /// this sub-heading instead of relying on a card title above it.
  heading?: string
  label: string
  pending: boolean
  onToggle: (enabled: boolean) => void
}) {
  const { t } = useTranslation("teacher")
  const older = useInfiniteQuery(
    ltiNrpsOlderRunsQuery(courseId, ctx.id, ctx.history.at(-1)?.ran_at ?? ""),
  )
  const loaded = [...ctx.history, ...(older.data?.pages.flat() ?? [])]
  const [shown, setShown] = useState(NRPS_HISTORY_PREVIEW)
  const history = loaded.slice(0, shown)
  const showOlder = () => {
    if (shown >= loaded.length) {
      older.fetchNextPage()
    }
    setShown((n) => n + NRPS_HISTORY_PAGE)
  }
  return (
    <div className={heading ? "space-y-2 rounded-md border p-3" : "space-y-2"}>
      {heading && (
        <div className="text-xs font-medium text-muted-foreground">
          {heading}
        </div>
      )}
      <div className="flex items-center gap-3">
        <Checkbox
          id={`nrps-sync-${ctx.id}`}
          checked={ctx.sync_enabled}
          disabled={pending}
          onCheckedChange={(checked) => onToggle(checked === true)}
        />
        <Label htmlFor={`nrps-sync-${ctx.id}`} className="cursor-pointer">
          {label}
        </Label>
      </div>
      <div className="min-w-0 space-y-1">
        <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
          {!ctx.sync_enabled ? (
            <Badge variant="outline">{t("lti.nrpsStatusOff")}</Badge>
          ) : ctx.last_sync_status === "error" ? (
            <Badge variant="destructive">{t("lti.nrpsStatusError")}</Badge>
          ) : ctx.last_sync_status === "ok" ? (
            <Badge variant="secondary">{t("lti.nrpsStatusOk")}</Badge>
          ) : (
            <Badge variant="outline">{t("lti.nrpsStatusPending")}</Badge>
          )}
          {ctx.last_sync_warning && (
            <Badge variant="outline" className="border-amber-500 text-amber-700 dark:text-amber-400">
              {t("lti.nrpsStatusWarning")}
            </Badge>
          )}
          {ctx.last_sync_at ? (
            <span className="text-xs text-muted-foreground">
              <RelativeTime date={ctx.last_sync_at} />
            </span>
          ) : (
            <span className="text-xs text-muted-foreground">
              {t("lti.nrpsNeverSynced")}
            </span>
          )}
          {ctx.total_added + ctx.total_removed > 0 && (
            <span className="text-xs text-muted-foreground">
              {t("lti.nrpsTotals", {
                added: ctx.total_added,
                removed: ctx.total_removed,
              })}
            </span>
          )}
        </div>
        {ctx.last_sync_status === "error" && ctx.last_sync_error && (
          <div className="text-xs text-destructive break-all">
            {ctx.last_sync_error}
          </div>
        )}
        {ctx.last_sync_warning && (
          <div className="text-xs text-amber-700 dark:text-amber-400 break-words">
            {ctx.last_sync_warning}
          </div>
        )}
      </div>
      {ctx.history.length > 0 && (
        <details className="text-xs">
          <summary className="cursor-pointer text-muted-foreground">
            {t("lti.nrpsHistoryTitle", { count: ctx.history_total })}
          </summary>
          <ul className="mt-2 ml-1 grid grid-cols-[auto_1fr] gap-x-4 gap-y-1 border-l pl-3">
            {history.map((run) => (
              <li key={run.id} className="col-span-2 grid grid-cols-subgrid">
                <span className="text-muted-foreground">
                  <RelativeTime date={run.ran_at} />
                </span>
                <div className="min-w-0 space-y-0.5">
                  {run.status === "error" ? (
                    <div className="text-destructive break-words">
                      {run.error ?? t("lti.nrpsStatusError")}
                    </div>
                  ) : (
                    <div>
                      {(run.added ?? 0) + (run.removed ?? 0) > 0
                        ? t("lti.nrpsCounts", {
                            added: run.added ?? 0,
                            removed: run.removed ?? 0,
                          })
                        : t("lti.nrpsHistoryNoChanges")}
                    </div>
                  )}
                  {run.warning && (
                    <div className="text-amber-700 dark:text-amber-400 break-words">
                      {run.warning}
                    </div>
                  )}
                </div>
              </li>
            ))}
          </ul>
          {shown < ctx.history_total && (
            <Button
              variant="ghost"
              size="sm"
              className="mt-1"
              onClick={showOlder}
              disabled={older.isFetching}
            >
              {t("lti.nrpsHistoryOlder")}
            </Button>
          )}
          {older.isError && <ErrorText error={older.error} />}
        </details>
      )}
    </div>
  )
}
