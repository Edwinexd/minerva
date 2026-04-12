import { createFileRoute } from "@tanstack/react-router"
import { useQuery } from "@tanstack/react-query"
import { adminSystemMetricsQuery } from "@/lib/queries"
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
          <CardTitle>System metrics unavailable</CardTitle>
          <CardDescription>
            {error instanceof Error ? error.message : "Unknown error"}
          </CardDescription>
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
          <CardTitle>Storage</CardTitle>
          <CardDescription>
            Filesystem backing{" "}
            <code className="text-xs">{disk?.path ?? "-"}</code> — the host
            volume holds postgres, qdrant, documents and backups. Contact DSV
            helpdesk to expand when free space runs low.
          </CardDescription>
        </CardHeader>
        <CardContent>
          {disk ? (
            <div className="space-y-3">
              <div className="flex items-baseline justify-between text-sm">
                <span className="font-medium">
                  {formatBytes(disk.used_bytes)} / {formatBytes(disk.total_bytes)} used
                </span>
                <span className="text-muted-foreground">
                  {formatBytes(disk.free_bytes)} free ({(100 - diskPct).toFixed(1)}%)
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
                    ? "Disk is nearly full — request expansion from helpdesk soon."
                    : "Disk is filling up — consider requesting expansion."}
                </p>
              )}
            </div>
          ) : (
            <p className="text-muted-foreground">
              Disk stats unavailable on this platform.
            </p>
          )}
        </CardContent>
      </Card>

      <div className="grid gap-4 md:grid-cols-2">
        <Card>
          <CardHeader className="pb-2">
            <CardDescription>PostgreSQL database size</CardDescription>
            <CardTitle className="text-2xl">
              {formatBytes(data.database.size_bytes)}
            </CardTitle>
          </CardHeader>
        </Card>
        <Card>
          <CardHeader className="pb-2">
            <CardDescription>Documents on disk</CardDescription>
            <CardTitle className="text-2xl">
              {formatBytes(data.documents.total_bytes)}
            </CardTitle>
          </CardHeader>
          <CardContent className="text-sm text-muted-foreground">
            {data.documents.count.toLocaleString()} total
            {data.documents.pending > 0 && (
              <> · {data.documents.pending.toLocaleString()} pending</>
            )}
            {data.documents.failed > 0 && (
              <>
                {" · "}
                <span className="text-red-600 dark:text-red-400">
                  {data.documents.failed.toLocaleString()} failed
                </span>
              </>
            )}
          </CardContent>
        </Card>
      </div>

      <Card>
        <CardHeader>
          <CardTitle className="flex items-center gap-2">
            Qdrant
            <Badge variant={data.qdrant.reachable ? "default" : "destructive"}>
              {data.qdrant.reachable ? "reachable" : "unreachable"}
            </Badge>
          </CardTitle>
          <CardDescription>Vector collections</CardDescription>
        </CardHeader>
        <CardContent>
          {data.qdrant.collections.length === 0 ? (
            <p className="text-muted-foreground">
              {data.qdrant.reachable
                ? "No collections yet."
                : "Could not connect to Qdrant."}
            </p>
          ) : (
            <div className="overflow-x-auto">
              <table className="w-full text-sm">
                <thead>
                  <tr className="border-b text-left">
                    <th className="py-2 pr-4 font-medium">Collection</th>
                    <th className="py-2 pr-4 font-medium text-right">Points</th>
                    <th className="py-2 pr-4 font-medium text-right">
                      Indexed vectors
                    </th>
                    <th className="py-2 font-medium text-right">Segments</th>
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
          <CardTitle>Database tables</CardTitle>
          <CardDescription>
            Approximate row counts from{" "}
            <code className="text-xs">pg_class.reltuples</code> (updated by
            ANALYZE, not exact).
          </CardDescription>
        </CardHeader>
        <CardContent>
          {data.database.table_counts.length === 0 ? (
            <p className="text-muted-foreground">No tables found.</p>
          ) : (
            <div className="overflow-x-auto">
              <table className="w-full text-sm">
                <thead>
                  <tr className="border-b text-left">
                    <th className="py-2 pr-4 font-medium">Table</th>
                    <th className="py-2 font-medium text-right">Rows (approx)</th>
                  </tr>
                </thead>
                <tbody>
                  {[...data.database.table_counts]
                    .sort((a, b) => b.rows - a.rows)
                    .map((t) => (
                      <tr key={t.name} className="border-b">
                        <td className="py-2 pr-4 font-mono text-xs">{t.name}</td>
                        <td className="py-2 text-right font-mono">
                          {t.rows.toLocaleString()}
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
