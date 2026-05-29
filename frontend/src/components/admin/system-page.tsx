import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query"
import { useTranslation } from "react-i18next"
import {
  adminClassificationStatsQuery,
  adminSystemMetricsQuery,
} from "@/lib/queries"
import { api } from "@/lib/api"
import { useApiErrorMessage } from "@/lib/use-api-error"
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { Skeleton } from "@/components/ui/skeleton"
import React from "react"

function formatBytes(bytes: number | null | undefined): string {
  if (bytes == null) return "-"
  if (bytes === 0) return "0 B"
  const units = ["B", "KB", "MB", "GB", "TB", "PB"]
  const i = Math.min(
    Math.floor(Math.log(Math.abs(bytes)) / Math.log(1024)),
    units.length - 1,
  )
  return `${(bytes / Math.pow(1024, i)).toFixed(i === 0 ? 0 : 2)} ${units[i]}`
}

export function SystemPanel() {
  const { t } = useTranslation("admin")
  const formatError = useApiErrorMessage()
  const { data, isLoading, error } = useQuery(adminSystemMetricsQuery)

  if (isLoading) {
    return (
      <div className="space-y-4">
        {Array.from({ length: 3 }).map((_, i) => (
          <Skeleton key={i} className="h-40 w-full" />
        ))}
      </div>
    )
  }

  if (error || !data) {
    return (
      <Card>
        <CardHeader>
          <CardTitle>{t("system.unavailableTitle")}</CardTitle>
          <CardDescription>{formatError(error)}</CardDescription>
        </CardHeader>
      </Card>
    )
  }

  const disk = data.disk
  const diskPct = disk && disk.total_bytes > 0
    ? (disk.used_bytes / disk.total_bytes) * 100
    : 0
  const diskTone =
    diskPct >= 90 ? "bg-red-600" : diskPct >= 75 ? "bg-amber-500" : "bg-emerald-600"

  return (
    <div className="space-y-6">
      <Card>
        <CardHeader>
          <CardTitle>{t("system.storage.title")}</CardTitle>
          <CardDescription>
            {t("system.storage.descriptionLead")}
            <code className="text-xs">{disk?.path ?? "-"}</code>
            {t("system.storage.descriptionTail")}
          </CardDescription>
        </CardHeader>
        <CardContent>
          {disk ? (
            <div className="space-y-3">
              <div className="flex items-baseline justify-between text-sm">
                <span className="font-medium">
                  {t("system.storage.usedOfTotal", {
                    used: formatBytes(disk.used_bytes),
                    total: formatBytes(disk.total_bytes),
                  })}
                </span>
                <span className="text-muted-foreground">
                  {t("system.storage.freePercent", {
                    free: formatBytes(disk.free_bytes),
                    percent: (100 - diskPct).toFixed(1),
                  })}
                </span>
              </div>
              <div className="h-3 w-full overflow-hidden rounded-full bg-muted">
                <div
                  className={`h-full ${diskTone} transition-all`}
                  style={{ width: `${Math.min(diskPct, 100).toFixed(2)}%` }}
                />
              </div>
              {diskPct >= 75 && (
                <p className="text-sm text-amber-700 dark:text-amber-400">
                  {diskPct >= 90
                    ? t("system.storage.nearlyFull")
                    : t("system.storage.fillingUp")}
                </p>
              )}
            </div>
          ) : (
            <p className="text-muted-foreground">
              {t("system.storage.statsUnavailable")}
            </p>
          )}
        </CardContent>
      </Card>

      <div className="grid gap-4 md:grid-cols-2">
        <Card>
          <CardHeader className="pb-2">
            <CardDescription>{t("system.databaseSize")}</CardDescription>
            <CardTitle className="text-2xl">
              {formatBytes(data.database.size_bytes)}
            </CardTitle>
          </CardHeader>
        </Card>
        <Card>
          <CardHeader className="pb-2">
            <CardDescription>{t("system.documentsOnDisk")}</CardDescription>
            <CardTitle className="text-2xl">
              {formatBytes(data.documents.total_bytes)}
            </CardTitle>
          </CardHeader>
          <CardContent className="text-sm text-muted-foreground">
            {t("system.documentsTotal", { value: data.documents.count.toLocaleString() })}
            {data.documents.pending > 0 && (
              <> · {t("system.documentsPending", { value: data.documents.pending.toLocaleString() })}</>
            )}
            {data.documents.failed > 0 && (
              <>
                {" · "}
                <span className="text-red-600 dark:text-red-400">
                  {t("system.documentsFailed", { value: data.documents.failed.toLocaleString() })}
                </span>
              </>
            )}
          </CardContent>
        </Card>
      </div>

      <Card>
        <CardHeader>
          <CardTitle className="flex items-center gap-2">
            {t("system.qdrant.title")}
            <Badge variant={data.qdrant.reachable ? "default" : "destructive"}>
              {data.qdrant.reachable ? t("system.qdrant.reachable") : t("system.qdrant.unreachable")}
            </Badge>
          </CardTitle>
          <CardDescription>{t("system.qdrant.description")}</CardDescription>
        </CardHeader>
        <CardContent>
          {data.qdrant.collections.length === 0 ? (
            <p className="text-muted-foreground">
              {data.qdrant.reachable
                ? t("system.qdrant.noCollections")
                : t("system.qdrant.notConnected")}
            </p>
          ) : (
            <div className="overflow-x-auto">
              <table className="w-full text-sm">
                <thead>
                  <tr className="border-b text-left">
                    <th className="py-2 pr-4 font-medium">{t("system.qdrant.columns.collection")}</th>
                    <th className="py-2 pr-4 font-medium text-right">{t("system.qdrant.columns.points")}</th>
                    <th className="py-2 pr-4 font-medium text-right">
                      {t("system.qdrant.columns.indexedVectors")}
                    </th>
                    <th className="py-2 font-medium text-right">{t("system.qdrant.columns.segments")}</th>
                  </tr>
                </thead>
                <tbody>
                  {data.qdrant.collections.map((c) => (
                    <tr key={c.name} className="border-b">
                      <td className="py-2 pr-4 font-mono text-xs">{c.name}</td>
                      <td className="py-2 pr-4 text-right font-mono">
                        {c.points_count?.toLocaleString() ?? "-"}
                      </td>
                      <td className="py-2 pr-4 text-right font-mono">
                        {c.indexed_vectors_count?.toLocaleString() ?? "-"}
                      </td>
                      <td className="py-2 text-right font-mono">
                        {c.segments_count?.toLocaleString() ?? "-"}
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          )}
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle>{t("system.databaseTables.title")}</CardTitle>
          <CardDescription>
            {t("system.databaseTables.descriptionLead")}
            <code className="text-xs">pg_class.reltuples</code>
            {t("system.databaseTables.descriptionTail")}
          </CardDescription>
        </CardHeader>
        <CardContent>
          {data.database.table_counts.length === 0 ? (
            <p className="text-muted-foreground">{t("system.databaseTables.empty")}</p>
          ) : (
            <div className="overflow-x-auto">
              <table className="w-full text-sm">
                <thead>
                  <tr className="border-b text-left">
                    <th className="py-2 pr-4 font-medium">{t("system.databaseTables.columns.table")}</th>
                    <th className="py-2 font-medium text-right">{t("system.databaseTables.columns.rowsApprox")}</th>
                  </tr>
                </thead>
                <tbody>
                  {[...data.database.table_counts]
                    .sort((a, b) => b.rows - a.rows)
                    .map((row) => (
                      <tr key={row.name} className="border-b">
                        <td className="py-2 pr-4 font-mono text-xs">{row.name}</td>
                        <td className="py-2 text-right font-mono">
                          {row.rows.toLocaleString()}
                        </td>
                      </tr>
                    ))}
                </tbody>
              </table>
            </div>
          )}
        </CardContent>
      </Card>

      <ClassificationBackfillCard />
    </div>
  )
}

/// Admin-scoped classification status + backfill trigger. Shows the
/// current eligible/done counts and lets an operator queue a batch.
/// The backend caps each click at BACKFILL_BATCH_LIMIT docs, so for
/// huge installations the admin re-clicks until `unclassified` reaches
/// zero (the polling stats query updates every 5s so progress is
/// visible without manual refresh).
function ClassificationBackfillCard() {
  const { t } = useTranslation("admin")
  const formatError = useApiErrorMessage()
  const queryClient = useQueryClient()
  const { data: stats, isLoading, error } = useQuery(adminClassificationStatsQuery)
  const [lastQueued, setLastQueued] = React.useState<number | null>(null)

  const backfillMutation = useMutation({
    mutationFn: () =>
      api.post<{ queued: number }>("/admin/backfill-classifications", {}),
    onSuccess: (data) => {
      setLastQueued(data.queued)
      queryClient.invalidateQueries({ queryKey: ["admin", "classification-stats"] })
    },
  })

  return (
    <Card>
      <CardHeader>
        <CardTitle>{t("system.classifications.title")}</CardTitle>
        <CardDescription>{t("system.classifications.description")}</CardDescription>
      </CardHeader>
      <CardContent>
        {isLoading ? (
          <Skeleton className="h-20 w-full" />
        ) : error || !stats ? (
          <p className="text-sm text-destructive">{formatError(error)}</p>
        ) : (
          <div className="space-y-4">
            <div className="grid grid-cols-2 gap-4 sm:grid-cols-4">
              <Stat
                label={t("system.classifications.totalReady")}
                value={stats.total_ready}
              />
              <Stat
                label={t("system.classifications.classified")}
                value={stats.classified}
                tone={stats.classified === stats.total_ready ? "good" : "neutral"}
              />
              <Stat
                label={t("system.classifications.unclassified")}
                value={stats.unclassified}
                tone={stats.unclassified > 0 ? "warn" : "good"}
              />
              <Stat
                label={t("system.classifications.lockedByTeacher")}
                value={stats.locked_by_teacher}
              />
            </div>
            <div className="flex flex-wrap items-center gap-3">
              <Button
                onClick={() => backfillMutation.mutate()}
                disabled={
                  backfillMutation.isPending ||
                  stats.unclassified === 0 ||
                  (stats.backfill != null && !stats.backfill.finished)
                }
                title={t("system.classifications.backfillTitle")}
              >
                {backfillMutation.isPending
                  ? t("system.classifications.backfilling")
                  : stats.unclassified === 0
                    ? t("system.classifications.backfillNoneNeeded")
                    : t("system.classifications.backfillButton", {
                        count: stats.unclassified,
                      })}
              </Button>
              {lastQueued != null && (
                <span className="text-sm text-muted-foreground">
                  {t("system.classifications.lastQueued", { count: lastQueued })}
                </span>
              )}
            </div>
            {/*
              Live progress of the running backfill. The backend
              tracker stays in `Some(_)` for one more poll cycle after
              the task drains, so the UI gets to flash the final
              "all done" state before the panel collapses back to the
              idle layout. Width caps at 100% defensively in case the
              candidate count grew between SELECT and processing.
            */}
            {stats.backfill && (
              <div className="space-y-1 rounded border p-3">
                <div className="flex items-baseline justify-between text-sm">
                  <span className="font-medium">
                    {stats.backfill.finished
                      ? t("system.classifications.backfillFinished", {
                          ok: stats.backfill.ok,
                          errors: stats.backfill.errors,
                          skipped: stats.backfill.skipped,
                        })
                      : t("system.classifications.backfillRunning", {
                          done: stats.backfill.ok + stats.backfill.errors + stats.backfill.skipped,
                          total: stats.backfill.total,
                        })}
                  </span>
                  <span className="text-xs text-muted-foreground tabular-nums">
                    {Math.round(
                      (Math.min(
                        stats.backfill.ok + stats.backfill.errors + stats.backfill.skipped,
                        stats.backfill.total,
                      ) /
                        Math.max(stats.backfill.total, 1)) *
                        100,
                    )}
                    %
                  </span>
                </div>
                <div className="h-2 w-full overflow-hidden rounded-full bg-muted">
                  <div
                    className={`h-full transition-all ${stats.backfill.errors > 0 ? "bg-amber-500" : "bg-emerald-600"}`}
                    style={{
                      width: `${Math.min(
                        ((stats.backfill.ok + stats.backfill.errors + stats.backfill.skipped) /
                          Math.max(stats.backfill.total, 1)) *
                          100,
                        100,
                      ).toFixed(1)}%`,
                    }}
                  />
                </div>
                {stats.backfill.errors > 0 && (
                  <p className="text-xs text-amber-700 dark:text-amber-400">
                    {t("system.classifications.backfillErrors", {
                      count: stats.backfill.errors,
                    })}
                  </p>
                )}
              </div>
            )}
            {backfillMutation.isError && (
              <p className="text-sm text-destructive">
                {formatError(backfillMutation.error)}
              </p>
            )}
            <p className="text-xs text-muted-foreground">
              {t("system.classifications.note")}
            </p>
          </div>
        )}
      </CardContent>
    </Card>
  )
}

function Stat({
  label,
  value,
  tone = "neutral",
}: {
  label: string
  value: number
  tone?: "good" | "warn" | "neutral"
}) {
  const toneClass =
    tone === "good"
      ? "text-emerald-700 dark:text-emerald-400"
      : tone === "warn"
        ? "text-amber-700 dark:text-amber-400"
        : "text-foreground"
  return (
    <div className="space-y-1">
      <div className={`text-2xl font-semibold tabular-nums ${toneClass}`}>
        {value.toLocaleString()}
      </div>
      <div className="text-xs text-muted-foreground">{label}</div>
    </div>
  )
}
