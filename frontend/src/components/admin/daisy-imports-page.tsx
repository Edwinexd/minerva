import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query"
import { useTranslation } from "react-i18next"
import { useMemo, useState } from "react"
import {
  daisyPendingQuery,
  type DaisyOfferingDiff,
  type DaisyPendingImport,
  type DaisyPendingListResponse,
} from "@/lib/queries"
import { api } from "@/lib/api"
import { useApiErrorMessage } from "@/lib/use-api-error"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card"
import { Checkbox } from "@/components/ui/checkbox"
import { Skeleton } from "@/components/ui/skeleton"
import { Label } from "@/components/ui/label"
import { RelativeTime } from "@/components/relative-time"

/**
 * Admin review page for Daisy auto-imports.
 *
 * The daily sync writes one row per Daisy course offering into
 * `daisy_pending_imports`; nothing reaches the live `courses` table
 * until an admin checks the box and clicks Apply. The toggle at the
 * top of this page flips `daisy_settings.auto_apply` so future syncs
 * skip staging entirely once we trust the workflow.
 *
 * Per-row "New" / "Update" badge mirrors `existing_course_id`: a
 * brand-new offering shows "New" (apply will INSERT into `courses`);
 * a row whose momenttillf_id is already in `courses` shows "Update"
 * (apply will refresh metadata + additively sync members on the
 * existing row).
 */
export function DaisyImportsPanel() {
  const { data, isLoading } = useQuery(daisyPendingQuery)

  if (isLoading) {
    return (
      <div className="space-y-2">
        {Array.from({ length: 5 }).map((_, i) => (
          <Skeleton key={i} className="h-14 w-full" />
        ))}
      </div>
    )
  }
  if (!data) return null

  return (
    <div className="space-y-6">
      <AutoApplyCard data={data} />
      <PendingTableCard data={data} />
    </div>
  )
}

function AutoApplyCard({ data }: { data: DaisyPendingListResponse }) {
  const { t } = useTranslation("admin")
  const formatError = useApiErrorMessage()
  const queryClient = useQueryClient()

  const mutation = useMutation({
    mutationFn: (enabled: boolean) =>
      api.put<{ auto_apply: boolean }>("/admin/daisy-settings/auto-apply", {
        enabled,
      }),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["admin", "daisy-pending"] })
    },
  })

  return (
    <Card>
      <CardHeader>
        <CardTitle>{t("daisyImports.autoApplyTitle")}</CardTitle>
        <CardDescription>
          {t("daisyImports.autoApplyDescription")}
        </CardDescription>
      </CardHeader>
      <CardContent>
        <div className="flex items-center gap-3">
          <Checkbox
            id="auto-apply"
            checked={data.auto_apply}
            disabled={mutation.isPending}
            onCheckedChange={(checked) => mutation.mutate(checked === true)}
          />
          <Label htmlFor="auto-apply" className="cursor-pointer">
            {data.auto_apply
              ? t("daisyImports.autoApplyOn")
              : t("daisyImports.autoApplyOff")}
          </Label>
        </div>
        {mutation.isError && (
          <p className="mt-2 text-sm text-destructive">
            {formatError(mutation.error)}
          </p>
        )}
      </CardContent>
    </Card>
  )
}

function PendingTableCard({ data }: { data: DaisyPendingListResponse }) {
  const { t } = useTranslation("admin")
  const formatError = useApiErrorMessage()
  const queryClient = useQueryClient()
  // Selection state lives in the page rather than in URL params; an
  // admin who navigates away and back probably wants a fresh list
  // (the underlying set may have changed mid-review anyway).
  const [selected, setSelected] = useState<Set<string>>(new Set())

  const applyMutation = useMutation({
    mutationFn: (ids: string[]) =>
      api.post<{
        courses_created: number
        courses_updated: number
        members_added: number
        aliases_registered: number
        errors: string[]
      }>("/admin/daisy-pending/apply", { ids }),
    onSuccess: () => {
      setSelected(new Set())
      queryClient.invalidateQueries({ queryKey: ["admin", "daisy-pending"] })
      // The applied rows turn into real courses; bust that cache too
      // so the My Courses page reflects them immediately.
      queryClient.invalidateQueries({ queryKey: ["courses"] })
    },
  })

  const dismissMutation = useMutation({
    mutationFn: (id: string) =>
      api.delete<{ deleted: boolean }>(`/admin/daisy-pending/${id}`),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["admin", "daisy-pending"] })
    },
  })

  // Group by semester for visual chunking; staging rows often arrive
  // in big VT/HT batches and seeing them mixed isn't useful.
  const groups = useMemo(() => groupBySemester(data.pending), [data.pending])

  const allIds = data.pending.map((p) => p.id)
  const allSelected =
    allIds.length > 0 && allIds.every((id) => selected.has(id))

  const toggleAll = () => {
    setSelected(allSelected ? new Set() : new Set(allIds))
  }

  const toggleOne = (id: string) => {
    setSelected((prev) => {
      const next = new Set(prev)
      if (next.has(id)) next.delete(id)
      else next.add(id)
      return next
    })
  }

  if (data.pending.length === 0) {
    return (
      <Card>
        <CardHeader>
          <CardTitle>{t("daisyImports.tableTitle", { count: 0 })}</CardTitle>
          <CardDescription>{t("daisyImports.emptyState")}</CardDescription>
        </CardHeader>
      </Card>
    )
  }

  return (
    <Card>
      <CardHeader>
        <div className="flex items-start justify-between gap-4">
          <div>
            <CardTitle>
              {t("daisyImports.tableTitle", { count: data.pending.length })}
            </CardTitle>
            <CardDescription>
              {t("daisyImports.tableDescription")}
            </CardDescription>
          </div>
          <div className="flex shrink-0 gap-2">
            <Button
              variant="outline"
              disabled={selected.size === 0 || applyMutation.isPending}
              onClick={() => applyMutation.mutate(Array.from(selected))}
            >
              {t("daisyImports.applySelected", { count: selected.size })}
            </Button>
            <Button
              disabled={applyMutation.isPending}
              onClick={() => applyMutation.mutate(allIds)}
            >
              {t("daisyImports.applyAll", { count: allIds.length })}
            </Button>
          </div>
        </div>
        {applyMutation.isError && (
          <p className="text-sm text-destructive">
            {formatError(applyMutation.error)}
          </p>
        )}
        {applyMutation.isSuccess && (
          <ApplyResultSummary result={applyMutation.data} />
        )}
      </CardHeader>
      <CardContent>
        <div className="overflow-x-auto">
          <table className="w-full border-collapse text-sm">
            <thead className="text-left text-xs uppercase text-muted-foreground">
              <tr className="border-b">
                <th className="w-10 px-2 py-2">
                  <Checkbox
                    checked={allSelected}
                    onCheckedChange={toggleAll}
                    aria-label={t("daisyImports.selectAllAria")}
                  />
                </th>
                <th className="px-2 py-2">{t("daisyImports.colCode")}</th>
                <th className="px-2 py-2">{t("daisyImports.colName")}</th>
                <th className="px-2 py-2">{t("daisyImports.colStatus")}</th>
                <th className="px-2 py-2">{t("daisyImports.colStaff")}</th>
                <th className="px-2 py-2">{t("daisyImports.colSeen")}</th>
                <th className="w-24 px-2 py-2"></th>
              </tr>
            </thead>
            <tbody>
              {groups.map(({ semester, rows }) => (
                <SemesterGroup
                  key={semester}
                  semester={semester}
                  rows={rows}
                  selected={selected}
                  onToggleOne={toggleOne}
                  onDismiss={(id) => dismissMutation.mutate(id)}
                />
              ))}
            </tbody>
          </table>
        </div>
      </CardContent>
    </Card>
  )
}

function SemesterGroup({
  semester,
  rows,
  selected,
  onToggleOne,
  onDismiss,
}: {
  semester: string
  rows: DaisyPendingImport[]
  selected: Set<string>
  onToggleOne: (id: string) => void
  onDismiss: (id: string) => void
}) {
  const { t } = useTranslation("admin")
  return (
    <>
      <tr className="bg-muted/50">
        <td colSpan={7} className="px-2 py-1 text-xs font-semibold uppercase">
          {semester || t("daisyImports.unsetSemester")}
        </td>
      </tr>
      {rows.map((row) => (
        <PendingRow
          key={row.id}
          row={row}
          checked={selected.has(row.id)}
          onToggle={() => onToggleOne(row.id)}
          onDismiss={() => onDismiss(row.id)}
        />
      ))}
    </>
  )
}

function PendingRow({
  row,
  checked,
  onToggle,
  onDismiss,
}: {
  row: DaisyPendingImport
  checked: boolean
  onToggle: () => void
  onDismiss: () => void
}) {
  const { t } = useTranslation("admin")
  return (
    <tr className="border-b align-top hover:bg-muted/30">
      <td className="px-2 py-2">
        <Checkbox
          checked={checked}
          onCheckedChange={onToggle}
          aria-label={t("daisyImports.selectRowAria", { name: row.name })}
        />
      </td>
      <td className="px-2 py-2 font-mono text-xs">{row.course_code}</td>
      <td className="px-2 py-2">
        <div>{row.name}</div>
        {row.daisy_info_url && (
          <a
            href={row.daisy_info_url}
            target="_blank"
            rel="noopener noreferrer"
            className="text-xs text-muted-foreground underline-offset-2 hover:underline"
          >
            {t("daisyImports.openInDaisy")}
          </a>
        )}
      </td>
      <td className="px-2 py-2">
        <DiffSummary diff={row.diff} />
      </td>
      <td className="px-2 py-2">
        <div>
          {t("daisyImports.staffCount", { count: row.participant_count })}
        </div>
        {row.participants
          .filter((p) =>
            p.daisy_roles.some(
              (r) =>
                r.toLowerCase().startsWith("kurs-/delkursansvarig") ||
                r.toLowerCase() === "kursansvarig",
            ),
          )
          .slice(0, 2)
          .map((p) => (
            <div
              key={p.eppns[0] ?? p.display_name ?? ""}
              className="text-xs text-muted-foreground"
            >
              {p.display_name ?? p.eppns[0]}
            </div>
          ))}
      </td>
      <td className="px-2 py-2 text-xs text-muted-foreground">
        <RelativeTime date={row.first_seen_at} />
      </td>
      <td className="px-2 py-2 text-right">
        <Button
          variant="ghost"
          size="sm"
          onClick={onDismiss}
          aria-label={t("daisyImports.dismissAria", { name: row.name })}
        >
          {t("daisyImports.dismiss")}
        </Button>
      </td>
    </tr>
  )
}

type TFn = (key: string, opts?: Record<string, unknown>) => string

function roleLabel(t: TFn, role: string): string {
  return role === "ta"
    ? t("daisyImports.roleTa")
    : t("daisyImports.roleTeacher")
}

function fieldLabel(t: TFn, field: string): string {
  switch (field) {
    case "name":
      return t("daisyImports.fieldName")
    case "course_code":
      return t("daisyImports.fieldCode")
    case "semester_label":
      return t("daisyImports.fieldSemester")
    case "info_url":
      return t("daisyImports.fieldInfoUrl")
    case "syllabus_url":
      return t("daisyImports.fieldSyllabus")
    case "unit":
      return t("daisyImports.fieldUnit")
    default:
      return field
  }
}

/**
 * Per-row status. The backend only returns rows whose apply would
 * actually change something, so this never renders an empty cell: a
 * brand-new offering shows the "New" badge, an update spells out the
 * specific member additions / role changes and the metadata fields a
 * re-apply would overwrite (old -> new in the chip's tooltip).
 */
function DiffSummary({ diff }: { diff: DaisyOfferingDiff }) {
  const { t } = useTranslation("admin")

  if (diff.is_new_course) {
    return <Badge variant="default">{t("daisyImports.statusNew")}</Badge>
  }

  return (
    <div className="space-y-1">
      {diff.member_changes.map((mc, i) => {
        const who = mc.display_name ?? mc.primary_eppn ?? "?"
        return (
          <div key={`m-${i}`} className="text-xs">
            {mc.change === "role_changed"
              ? t("daisyImports.diffMemberRole", {
                  name: who,
                  from: roleLabel(t, mc.previous_role ?? ""),
                  to: roleLabel(t, mc.role),
                })
              : t("daisyImports.diffMemberAdded", {
                  name: who,
                  role: roleLabel(t, mc.role),
                })}
          </div>
        )
      })}
      {diff.metadata_changes.length > 0 && (
        <div className="flex flex-wrap gap-1">
          {diff.metadata_changes.map((fc) => (
            <Badge
              key={fc.field}
              variant="secondary"
              title={t("daisyImports.diffFieldDetail", {
                field: fieldLabel(t, fc.field),
                from: fc.old ?? t("daisyImports.noValue"),
                to: fc.new ?? t("daisyImports.noValue"),
              })}
            >
              {fieldLabel(t, fc.field)}
            </Badge>
          ))}
        </div>
      )}
    </div>
  )
}

function ApplyResultSummary({
  result,
}: {
  result: {
    courses_created: number
    courses_updated: number
    members_added: number
    aliases_registered: number
    errors: string[]
  }
}) {
  const { t } = useTranslation("admin")
  return (
    <div className="mt-2 rounded border border-border bg-muted/30 p-3 text-sm">
      <div>
        {t("daisyImports.applyResult", {
          created: result.courses_created,
          updated: result.courses_updated,
          members: result.members_added,
          aliases: result.aliases_registered,
        })}
      </div>
      {result.errors.length > 0 && (
        <details className="mt-2">
          <summary className="cursor-pointer text-destructive">
            {t("daisyImports.applyErrorsHeading", {
              count: result.errors.length,
            })}
          </summary>
          <ul className="mt-1 list-disc pl-5 text-xs text-destructive">
            {result.errors.map((e, i) => (
              <li key={i}>{e}</li>
            ))}
          </ul>
        </details>
      )}
    </div>
  )
}

/**
 * Bucket pending rows by `semester_label`, newest-semester-first.
 * Empty/missing labels (shouldn't happen post-validation, but be
 * defensive) sort to the end.
 */
function groupBySemester(
  rows: DaisyPendingImport[],
): Array<{ semester: string; rows: DaisyPendingImport[] }> {
  const buckets = new Map<string, DaisyPendingImport[]>()
  for (const r of rows) {
    const key = r.semester_label || ""
    if (!buckets.has(key)) buckets.set(key, [])
    buckets.get(key)!.push(r)
  }
  const entries = Array.from(buckets.entries()).map(([semester, rs]) => ({
    semester,
    rows: rs,
  }))
  entries.sort((a, b) => semesterSortKey(b.semester) - semesterSortKey(a.semester))
  return entries
}

function semesterSortKey(label: string): number {
  if (!label) return -Infinity
  const m = label.match(/^(VT|HT)(\d{4})$/)
  if (!m) return -Infinity
  const year = parseInt(m[2], 10)
  const seasonOffset = m[1] === "HT" ? 0.5 : 0
  return year + seasonOffset
}
