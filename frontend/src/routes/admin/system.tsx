import { createFileRoute } from "@tanstack/react-router"
import { useQuery } from "@tanstack/react-query"
import { useTranslation } from "react-i18next"
import { adminSystemMetricsQuery } from "@/lib/queries"
import { useApiErrorMessage } from "@/lib/use-api-error"
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card"
import { Badge } from "@/components/ui/badge"
import { Skeleton } from "@/components/ui/skeleton"

export const Route = createFileRoute("/admin/system")({
  component: SystemPanel,
})

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

function SystemPanel() {
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
    </div>
  )
}
