/**
 * /admin/dev-tools - dev-mode-only admin panel for filling the DB with
 * fixture data. Same destructive reseed code path as
 * `scripts/seed-dev.sh`; the page is here so the operator doesn't have
 * to shell out mid-session to refresh the dataset.
 *
 * The whole panel is gated twice over: the surrounding admin-layout
 * filters this tab out unless `devConfig.dev_mode === true`, and the
 * server's POST /admin/dev/seed returns 404 in prod. The body still
 * shows a "dev mode not enabled" notice if a user somehow lands here
 * directly via URL outside dev mode, so the page isn't a blank-screen
 * mystery in that case.
 */
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query"
import { useState } from "react"
import { useTranslation } from "react-i18next"
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

interface DevConfig {
  dev_mode: boolean
}

interface WipeReport {
  messages: number
  conversations: number
  documents: number
  course_members: number
  external_invites: number
  courses: number
  users: number
  course_dirs_removed: number
  qdrant_collections_removed: number
}

interface SeedReport {
  admin_eppn: string
  admin_user_id: string
  users: number
  courses: number
  course_members: number
  documents: number
  conversations: number
  messages: number
  external_invites: number
  wiped: WipeReport
}

// Eppns the seeder always creates. Mirrored from
// backend/crates/minerva-server/src/dev_seed.rs. Kept here as a flat
// list (rather than fetched from the seed report) so the operator
// can SEE the cast before clicking Reseed, not just after.
const SEEDED_USERS: { eppn: string; role: string; notes?: string }[] = [
  { eppn: "seed-teacher@dev.local", role: "teacher" },
  { eppn: "seed-integrator@dev.local", role: "integrator" },
  { eppn: "seed-alice@dev.local", role: "student" },
  { eppn: "seed-bob@dev.local", role: "student" },
  { eppn: "seed-carol@dev.local", role: "student" },
  { eppn: "seed-dan@dev.local", role: "student", notes: "TA in 1 course" },
  { eppn: "ext:seed-guest@dev.local", role: "student", notes: "external invite" },
]

const SEEDED_COURSES: { name: string; config: string; ownership: string }[] = [
  {
    name: "Intro Programming (seed)",
    config: "simple",
    ownership: "owned by you (the admin)",
  },
  {
    name: "Advanced Algorithms (seed)",
    config: "FLARE",
    ownership: "owned by you (the admin)",
  },
  {
    name: "Web Development (seed)",
    config: "FLARE + tool use (agentic)",
    ownership: "owned by seed-teacher; you are enrolled as a student",
  },
  {
    name: "Database Systems (seed)",
    config: "simple",
    ownership: "owned by seed-teacher; you are NOT enrolled",
  },
]

export function DevToolsPanel() {
  const { t } = useTranslation("admin")
  const queryClient = useQueryClient()
  const formatError = useApiErrorMessage()

  const { data: devConfig, isLoading: devLoading } = useQuery({
    queryKey: ["dev", "config"],
    queryFn: () => api.get<DevConfig>("/dev/config"),
    staleTime: Infinity,
  })

  const [lastReport, setLastReport] = useState<SeedReport | null>(null)

  const seed = useMutation({
    mutationFn: () => api.post<SeedReport>("/admin/dev/seed", {}),
    onSuccess: (report) => {
      setLastReport(report)
      // Anything the operator might be looking at next refetches
      // from scratch. Course lists, conversation lists, user lists,
      // documents lists all key off the seeded rows.
      queryClient.invalidateQueries({ queryKey: ["courses"] })
      queryClient.invalidateQueries({ queryKey: ["admin", "users"] })
      queryClient.invalidateQueries({ queryKey: ["admin", "external-invites"] })
    },
  })

  if (devLoading) {
    return <p className="text-sm text-muted-foreground">{t("devTools.loading")}</p>
  }

  if (!devConfig?.dev_mode) {
    return (
      <Card>
        <CardHeader>
          <CardTitle>{t("devTools.notDevTitle")}</CardTitle>
          <CardDescription>{t("devTools.notDevDescription")}</CardDescription>
        </CardHeader>
      </Card>
    )
  }

  return (
    <div className="space-y-6">
      <Card>
        <CardHeader>
          <CardTitle>{t("devTools.reseedTitle")}</CardTitle>
          <CardDescription>{t("devTools.reseedDescription")}</CardDescription>
        </CardHeader>
        <CardContent className="space-y-4">
          <div className="rounded-md border border-rose-300 bg-rose-50 p-3 text-sm dark:border-rose-800 dark:bg-rose-950/40">
            <strong>{t("devTools.destructiveLabel")}</strong>{" "}
            {t("devTools.destructiveBody")}
          </div>

          <div className="flex flex-wrap items-center gap-3">
            <Button
              type="button"
              onClick={() => seed.mutate()}
              disabled={seed.isPending}
            >
              {seed.isPending ? t("devTools.reseeding") : t("devTools.reseedButton")}
            </Button>
            {seed.isError && (
              <p role="alert" className="text-sm text-destructive">
                {formatError(seed.error)}
              </p>
            )}
          </div>

          {lastReport && <ReseedReport report={lastReport} />}
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle>{t("devTools.fixtureUsersTitle")}</CardTitle>
          <CardDescription>{t("devTools.fixtureUsersDescription")}</CardDescription>
        </CardHeader>
        <CardContent>
          <div className="overflow-x-auto">
            <table className="w-full text-sm">
              <thead>
                <tr className="border-b text-left">
                  <th className="py-2 pr-4 font-medium">{t("devTools.columns.eppn")}</th>
                  <th className="py-2 pr-4 font-medium">{t("devTools.columns.role")}</th>
                  <th className="py-2 font-medium">{t("devTools.columns.notes")}</th>
                </tr>
              </thead>
              <tbody>
                {SEEDED_USERS.map((u) => (
                  <tr key={u.eppn} className="border-b last:border-0">
                    <td className="py-2 pr-4 font-mono text-xs">{u.eppn}</td>
                    <td className="py-2 pr-4">{u.role}</td>
                    <td className="py-2 text-muted-foreground text-xs">{u.notes ?? ""}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
          <p className="mt-3 text-xs text-muted-foreground">
            {t("devTools.fixtureUsersTip")}
          </p>
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle>{t("devTools.fixtureCoursesTitle")}</CardTitle>
          <CardDescription>{t("devTools.fixtureCoursesDescription")}</CardDescription>
        </CardHeader>
        <CardContent>
          <ul className="space-y-3 text-sm">
            {SEEDED_COURSES.map((c) => (
              <li key={c.name} className="rounded border p-3">
                <div className="font-medium">{c.name}</div>
                <div className="text-xs text-muted-foreground">
                  <span className="font-mono">{c.config}</span> &middot; {c.ownership}
                </div>
              </li>
            ))}
          </ul>
        </CardContent>
      </Card>
    </div>
  )
}

function ReseedReport({ report }: { report: SeedReport }) {
  const { t } = useTranslation("admin")
  return (
    <div className="rounded-md border border-emerald-300 bg-emerald-50 p-3 text-sm dark:border-emerald-800 dark:bg-emerald-950/40">
      <p className="mb-2 font-medium">
        {t("devTools.report.successHeading", { eppn: report.admin_eppn })}
      </p>
      <div className="grid gap-3 sm:grid-cols-2">
        <div>
          <p className="text-xs font-semibold uppercase tracking-wide text-muted-foreground">
            {t("devTools.report.wipedHeading")}
          </p>
          <ul className="mt-1 space-y-0.5 text-xs">
            <li>{t("devTools.report.users", { n: report.wiped.users })}</li>
            <li>{t("devTools.report.courses", { n: report.wiped.courses })}</li>
            <li>{t("devTools.report.documents", { n: report.wiped.documents })}</li>
            <li>{t("devTools.report.members", { n: report.wiped.course_members })}</li>
            <li>{t("devTools.report.conversations", { n: report.wiped.conversations })}</li>
            <li>{t("devTools.report.messages", { n: report.wiped.messages })}</li>
            <li>{t("devTools.report.invites", { n: report.wiped.external_invites })}</li>
            <li>{t("devTools.report.dirs", { n: report.wiped.course_dirs_removed })}</li>
            <li>{t("devTools.report.qdrant", { n: report.wiped.qdrant_collections_removed })}</li>
          </ul>
        </div>
        <div>
          <p className="text-xs font-semibold uppercase tracking-wide text-muted-foreground">
            {t("devTools.report.seededHeading")}
          </p>
          <ul className="mt-1 space-y-0.5 text-xs">
            <li>{t("devTools.report.users", { n: report.users })}</li>
            <li>{t("devTools.report.courses", { n: report.courses })}</li>
            <li>{t("devTools.report.documents", { n: report.documents })}</li>
            <li>{t("devTools.report.members", { n: report.course_members })}</li>
            <li>{t("devTools.report.conversations", { n: report.conversations })}</li>
            <li>{t("devTools.report.messages", { n: report.messages })}</li>
            <li>{t("devTools.report.invites", { n: report.external_invites })}</li>
          </ul>
        </div>
      </div>
      <p className="mt-2 text-xs text-muted-foreground">
        {t("devTools.report.embeddingNote")}
      </p>
    </div>
  )
}
