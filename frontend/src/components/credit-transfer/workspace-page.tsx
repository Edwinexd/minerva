import { useEffect, useState } from "react"
import { Link, useNavigate } from "@tanstack/react-router"
import { keepPreviousData, useQuery } from "@tanstack/react-query"
import { useTranslation } from "react-i18next"
import {
  creditTransferCasesQuery,
  creditTransferSummaryQuery,
  dsvPlansQuery,
} from "@/lib/queries"
import type { Course, PlanStatus } from "@/lib/types"
import { useDocumentTitle } from "@/lib/use-document-title"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { Skeleton } from "@/components/ui/skeleton"
import { ErrorText } from "@/components/ui/error-text"
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs"
import { AssistantsPage } from "@/components/teacher/assistants-page"
import { StatusBadge } from "./assessment-view"
import { useDateFormat } from "./dates"
import { WORKSPACE_TABS, type WorkspaceTab } from "./tabs"
import { VERDICT_TONE } from "./tones"

/**
 * A credit transfer workspace: the viewer's own cases, the DSV course
 * plans they are compared against, and connecting an AI assistant. The
 * workspace settings stay on the teacher pages, linked for its managers.
 */
export function CreditTransferWorkspacePage({
  workspace,
  tab,
}: {
  workspace: Course
  tab: WorkspaceTab
}) {
  const { t } = useTranslation("student")
  const navigate = useNavigate()
  useDocumentTitle(workspace.name)
  const manages = workspace.my_role === "teacher" || workspace.my_role === "ta"
  const { data: summary } = useQuery(creditTransferSummaryQuery(workspace.id))

  return (
    <div className="space-y-4">
      <div className="space-y-1">
        <h1 className="text-2xl font-semibold">{workspace.name}</h1>
        <p className="text-muted-foreground">
          {t("creditTransfer.intro", { days: summary?.case_retention_days ?? 90 })}
        </p>
      </div>
      <Tabs
        value={tab}
        onValueChange={(value: unknown) => {
          if (typeof value === "string" && (WORKSPACE_TABS as readonly string[]).includes(value)) {
            navigate({
              to: "/course/$courseId",
              params: { courseId: workspace.id },
              search: { tab: value as WorkspaceTab },
              replace: true,
            })
          }
        }}
      >
        <div className="flex flex-wrap items-center justify-between gap-2 border-b pb-2">
          <TabsList>
            <TabsTrigger value="cases">{t("creditTransfer.tabs.cases")}</TabsTrigger>
            <TabsTrigger value="plans">{t("creditTransfer.tabs.plans")}</TabsTrigger>
            <TabsTrigger value="assistant">{t("creditTransfer.tabs.assistant")}</TabsTrigger>
          </TabsList>
          {manages && (
            <Link
              to="/teacher/courses/$courseId"
              params={{ courseId: workspace.id }}
              className="text-sm text-muted-foreground underline hover:text-foreground"
            >
              {t("creditTransfer.tabs.settings")}
            </Link>
          )}
        </div>
        <TabsContent value="cases" className="pt-2">
          <CasesTab workspaceId={workspace.id} />
        </TabsContent>
        <TabsContent value="plans" className="pt-2">
          <PlansTab workspaceId={workspace.id} />
        </TabsContent>
        <TabsContent value="assistant" className="pt-2">
          <AssistantsPage />
        </TabsContent>
      </Tabs>
    </div>
  )
}

function CasesTab({ workspaceId }: { workspaceId: string }) {
  const { t } = useTranslation("student")
  const navigate = useNavigate()
  const formatDate = useDateFormat()
  const { data: cases, isLoading, error } = useQuery(creditTransferCasesQuery(workspaceId))

  return (
    <div className="space-y-3">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <p className="text-sm text-muted-foreground">{t("creditTransfer.casesPrivate")}</p>
        <Button
          type="button"
          onClick={() => navigate({ to: "/course/$courseId/new", params: { courseId: workspaceId } })}
        >
          {t("creditTransfer.newCase")}
        </Button>
      </div>
      {isLoading && <Skeleton className="h-32 w-full" />}
      {error && <ErrorText error={error} />}
      {cases && cases.length === 0 && (
        <p className="text-sm text-muted-foreground">{t("creditTransfer.noCases")}</p>
      )}
      {cases && cases.length > 0 && (
        // Scrolls on a narrow screen (and the plan list always), so the
        // region takes focus for keyboard users (see chat-surface.tsx).
        <section
          className="overflow-x-auto"
          // eslint-disable-next-line jsx-a11y/no-noninteractive-tabindex
          tabIndex={0}
          aria-label={t("creditTransfer.casesTable")}
        >
          <table className="w-full border-collapse text-sm">
            <thead>
              <tr className="border-b text-left text-muted-foreground">
                <th scope="col" className="py-2 pr-3 font-medium">{t("creditTransfer.col.case")}</th>
                <th scope="col" className="py-2 pr-3 font-medium">{t("creditTransfer.col.files")}</th>
                <th scope="col" className="py-2 pr-3 font-medium">{t("creditTransfer.col.bestMatch")}</th>
                <th scope="col" className="py-2 pr-3 font-medium">{t("creditTransfer.col.updated")}</th>
                <th scope="col" className="py-2 font-medium">{t("creditTransfer.col.deletedOn")}</th>
              </tr>
            </thead>
            <tbody>
              {cases.map((c) => (
                <tr key={c.id} className="border-b align-top last:border-b-0">
                  <td className="py-2 pr-3">
                    <Link
                      to="/course/$courseId/$conversationId"
                      params={{ courseId: workspaceId, conversationId: c.id }}
                      className="font-medium hover:underline"
                    >
                      {c.title ?? t("creditTransfer.untitledCase")}
                    </Link>
                  </td>
                  <td className="py-2 pr-3 text-muted-foreground">
                    {c.attachments.length > 0 ? c.attachments.join(", ") : "-"}
                  </td>
                  <td className="py-2 pr-3">
                    {c.best_match ? (
                      <span className="inline-flex flex-wrap items-center gap-1.5">
                        {c.best_match.code}
                        <StatusBadge tone={VERDICT_TONE[c.best_match.verdict]}>
                          {t(`creditTransfer.verdictShort.${c.best_match.verdict}`)}
                        </StatusBadge>
                      </span>
                    ) : (
                      <span className="text-muted-foreground">
                        {c.attachments.length > 0
                          ? t("creditTransfer.notAssessed")
                          : t("creditTransfer.noFiles")}
                      </span>
                    )}
                  </td>
                  <td className="py-2 pr-3 whitespace-nowrap">{formatDate(c.updated_at)}</td>
                  <td className="py-2 whitespace-nowrap text-muted-foreground">
                    {formatDate(c.deleted_on)}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </section>
      )}
    </div>
  )
}

const STATUS_TONE: Record<PlanStatus, "ok" | "info" | "neutral" | "bad"> = {
  indexed: "ok",
  indexing: "info",
  pending: "neutral",
  failed: "bad",
  discontinued: "neutral",
}

function PlansTab({ workspaceId }: { workspaceId: string }) {
  const { t, i18n } = useTranslation("student")
  const formatDate = useDateFormat()
  const [query, setQuery] = useState("")
  // Searched server side (the plans' text is not sent to the browser),
  // a moment after typing stops; the previous result stays up meanwhile.
  const [search, setSearch] = useState("")
  useEffect(() => {
    const timer = setTimeout(() => setSearch(query.trim()), 250)
    return () => clearTimeout(timer)
  }, [query])
  const {
    data: plans,
    isLoading,
    error,
  } = useQuery({ ...dsvPlansQuery(workspaceId, search), placeholderData: keepPreviousData })
  const { data: summary } = useQuery(creditTransferSummaryQuery(workspaceId))
  const credits = new Intl.NumberFormat(i18n.language, { maximumFractionDigits: 1 })
  const english = i18n.language.startsWith("en")
  const shown = plans ?? []

  return (
    <div className="space-y-3">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <Input
          type="search"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          placeholder={t("creditTransfer.plans.search")}
          aria-label={t("creditTransfer.plans.search")}
          className="max-w-sm"
        />
        {summary && (
          <p className="text-sm text-muted-foreground">
            {summary.plans_checked_at
              ? t("creditTransfer.plans.syncStatus", {
                  date: formatDate(summary.plans_checked_at),
                  count: summary.plan_count,
                  changed: summary.plans_changed_this_week,
                })
              : t("creditTransfer.plans.neverSynced")}
          </p>
        )}
      </div>
      <p className="text-xs text-muted-foreground">{t("creditTransfer.plans.readOnly")}</p>
      {isLoading && <Skeleton className="h-48 w-full" />}
      {error && <ErrorText error={error} />}
      {plans && (
        // Scrolls on a narrow screen (and the plan list always), so the
        // region takes focus for keyboard users (see chat-surface.tsx).
        <section
          className="max-h-[60vh] overflow-auto"
          // eslint-disable-next-line jsx-a11y/no-noninteractive-tabindex
          tabIndex={0}
          aria-label={t("creditTransfer.plans.table")}
        >
          <table className="w-full border-collapse text-sm">
            <thead className="sticky top-0 bg-background">
              <tr className="border-b text-left text-muted-foreground">
                <th scope="col" className="py-2 pr-3 font-medium">{t("creditTransfer.plans.code")}</th>
                <th scope="col" className="py-2 pr-3 font-medium">{t("creditTransfer.plans.name")}</th>
                <th scope="col" className="py-2 pr-3 font-medium">{t("creditTransfer.plans.creditsLevel")}</th>
                <th scope="col" className="py-2 pr-3 font-medium">{t("creditTransfer.plans.latest")}</th>
                <th scope="col" className="py-2 pr-3 font-medium">{t("creditTransfer.plans.older")}</th>
                <th scope="col" className="py-2 font-medium">{t("creditTransfer.plans.status")}</th>
              </tr>
            </thead>
            <tbody>
              {shown.map((p) => (
                <tr key={p.code} className="border-b align-top last:border-b-0">
                  <td className="py-2 pr-3 font-medium">{p.code}</td>
                  <td className="py-2 pr-3">{(english && p.name_en) || p.name_sv}</td>
                  <td className="py-2 pr-3 whitespace-nowrap">
                    {[p.ects != null ? credits.format(p.ects) : null, p.level]
                      .filter(Boolean)
                      .join(" / ") || "-"}
                  </td>
                  <td className="py-2 pr-3">
                    {p.latest ? (
                      <>
                        {p.latest.valid_from}{" "}
                        <span className="text-xs text-muted-foreground">
                          (
                          {p.latest.files.map((f, i) => (
                            <span key={f.language}>
                              {i > 0 && ", "}
                              <a
                                href={f.pdf_url}
                                target="_blank"
                                rel="noopener noreferrer"
                                className="underline hover:text-foreground"
                                aria-label={t("creditTransfer.plans.openPdf", {
                                  code: p.code,
                                  version: p.latest!.valid_from,
                                  language: f.language,
                                })}
                              >
                                {f.language}
                              </a>
                            </span>
                          ))}
                          )
                        </span>
                      </>
                    ) : (
                      "-"
                    )}
                  </td>
                  <td className="py-2 pr-3 text-muted-foreground">
                    {p.older_versions.length > 0
                      ? p.older_versions.join(", ")
                      : t("creditTransfer.plans.none")}
                  </td>
                  <td className="py-2">
                    <span className="inline-flex flex-wrap gap-1">
                      <StatusBadge tone={STATUS_TONE[p.status]}>
                        {t(`creditTransfer.plans.statuses.${p.status}`)}
                      </StatusBadge>
                      {p.changed_this_week && p.status !== "discontinued" && (
                        <StatusBadge tone="info">{t("creditTransfer.plans.newThisWeek")}</StatusBadge>
                      )}
                    </span>
                  </td>
                </tr>
              ))}
              {shown.length === 0 && (
                <tr>
                  <td colSpan={6} className="py-3 text-muted-foreground">
                    {t("creditTransfer.plans.noMatch")}
                  </td>
                </tr>
              )}
            </tbody>
          </table>
        </section>
      )}
    </div>
  )
}
