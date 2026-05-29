import { useMutation, useQueryClient } from "@tanstack/react-query"
import { useTranslation } from "react-i18next"
import React from "react"

import { api } from "@/lib/api"
import { useApiErrorMessage } from "@/lib/use-api-error"
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card"
import { Button } from "@/components/ui/button"
import { Checkbox } from "@/components/ui/checkbox"
import { Skeleton } from "@/components/ui/skeleton"
import { Badge } from "@/components/ui/badge"
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog"

/// Fields every admin model-catalog row carries, regardless of model
/// kind. Per-catalog extras (dimensions, warmed_at_startup, the typed
/// benchmark shape) are reached through the `extraColumns` / `speedOf` /
/// `renderModelName` props rather than widening this.
export interface BaseModelRow {
  model: string
  enabled: boolean
  is_default: boolean
  courses_using: number
}

export interface ModelCatalogExtraColumn<Row> {
  /// Suffix under `<i18nPrefix>.columns`, e.g. "dimensions".
  headerKey: string
  render: (m: Row) => React.ReactNode
}

/// Shared admin catalog table for embedding + re-ranker models.
///
/// Both catalogs render the identical table - enable toggle (with an
/// in-use confirm gate), single-default radio, model name + badges,
/// optional model-specific columns, a benchmark Speed column, a
/// courses-using count, and a per-row Run benchmark button - so keeping
/// them in one component is what guarantees the columns stay in the same
/// order with the same spacing. The differences (i18n namespace, API
/// paths, model-name rendering, the embedding-only dimensions column,
/// the benchmark throughput field) come in as props.
export function ModelCatalogCard<Row extends BaseModelRow>({
  i18nPrefix,
  data,
  isLoading,
  error,
  adminQueryKey,
  pickerQueryKey,
  benchmarkQueryKey,
  enabledPath,
  defaultPath,
  benchmarkPath,
  defaultRadioName,
  renderModelName,
  extraColumns = [],
  speedOf,
}: {
  i18nPrefix: string
  data: { models: Row[]; running: boolean } | undefined
  isLoading: boolean
  error: unknown
  adminQueryKey: readonly unknown[]
  /// Picker feed invalidated when an enable flag flips (so the teacher
  /// dropdown updates without a hard refresh).
  pickerQueryKey: readonly unknown[]
  /// Optional public benchmark feed invalidated after a run (embedding
  /// has one; the re-ranker doesn't).
  benchmarkQueryKey?: readonly unknown[]
  enabledPath: string
  defaultPath: string
  benchmarkPath: string
  defaultRadioName: string
  renderModelName: (m: Row) => React.ReactNode
  extraColumns?: ModelCatalogExtraColumn<Row>[]
  /// Benchmark throughput for the Speed column, or null if not run yet.
  speedOf: (m: Row) => number | null
}) {
  const { t } = useTranslation("admin")
  const formatError = useApiErrorMessage()
  const queryClient = useQueryClient()
  const [pendingModel, setPendingModel] = React.useState<string | null>(null)
  // Confirmation gate for disabling a model that courses still use.
  const [confirmDisable, setConfirmDisable] = React.useState<{
    model: string
    coursesUsing: number
  } | null>(null)

  // Namespaced translation helper so call sites read `tx("columns.model")`
  // instead of repeating the prefix.
  const tx = (suffix: string, opts?: Record<string, unknown>) =>
    t(`${i18nPrefix}.${suffix}`, opts ?? {})

  const benchmarkMutation = useMutation({
    mutationFn: (model: string) =>
      api.post<{ result: unknown }>(benchmarkPath, { model }),
    onMutate: (model) => setPendingModel(model),
    onSettled: () => {
      setPendingModel(null)
      queryClient.invalidateQueries({ queryKey: adminQueryKey })
      if (benchmarkQueryKey) {
        queryClient.invalidateQueries({ queryKey: benchmarkQueryKey })
      }
    },
  })

  const defaultMutation = useMutation({
    mutationFn: (model: string) =>
      api.put<{ model: string; is_default: boolean }>(defaultPath, { model }),
    onSettled: () => queryClient.invalidateQueries({ queryKey: adminQueryKey }),
  })

  const enabledMutation = useMutation({
    // Model id in the body, not the path: HuggingFace-style ids contain
    // forward slashes that axum path-routing collapses.
    mutationFn: ({ model, enabled }: { model: string; enabled: boolean }) =>
      api.put<{ model: string; enabled: boolean }>(enabledPath, {
        model,
        enabled,
      }),
    onSettled: () => {
      queryClient.invalidateQueries({ queryKey: adminQueryKey })
      queryClient.invalidateQueries({ queryKey: pickerQueryKey })
    },
  })

  return (
    <Card>
      <CardHeader>
        <CardTitle>{tx("title")}</CardTitle>
        <CardDescription>{tx("description")}</CardDescription>
      </CardHeader>
      <CardContent>
        {isLoading ? (
          <Skeleton className="h-40 w-full" />
        ) : error || !data ? (
          <p className="text-sm text-destructive">{formatError(error)}</p>
        ) : (
          <div className="space-y-3">
            {data.running && (
              <p className="text-xs text-muted-foreground">
                {tx("runningHint", { model: pendingModel ?? "" })}
              </p>
            )}
            <div className="overflow-x-auto">
              <table className="w-full text-sm">
                <thead>
                  <tr className="border-b text-left">
                    <th className="py-2 pr-4 font-medium">
                      {tx("columns.enabled")}
                    </th>
                    <th className="py-2 pr-4 font-medium">
                      {tx("columns.default")}
                    </th>
                    <th className="py-2 pr-4 font-medium">
                      {tx("columns.model")}
                    </th>
                    {extraColumns.map((col) => (
                      <th
                        key={col.headerKey}
                        className="py-2 pr-4 font-medium text-right"
                      >
                        {tx(`columns.${col.headerKey}`)}
                      </th>
                    ))}
                    <th className="py-2 pr-4 font-medium text-right">
                      {tx("columns.speed")}
                    </th>
                    <th className="py-2 pr-4 font-medium text-right">
                      {tx("columns.coursesUsing")}
                    </th>
                    <th className="py-2 font-medium text-right">
                      {tx("columns.action")}
                    </th>
                  </tr>
                </thead>
                <tbody>
                  {data.models.map((m) => {
                    const isThisRunning =
                      data.running && pendingModel === m.model
                    const speed = speedOf(m)
                    return (
                      <tr key={m.model} className="border-b">
                        <td className="py-2 pr-4">
                          <Checkbox
                            checked={m.enabled}
                            disabled={enabledMutation.isPending || m.is_default}
                            onCheckedChange={(value) => {
                              const next = value === true
                              // Disabling a model in use: confirm first so
                              // the admin sees the impact. Enabling, or
                              // disabling an unused model, saves immediately.
                              if (!next && m.courses_using > 0) {
                                setConfirmDisable({
                                  model: m.model,
                                  coursesUsing: m.courses_using,
                                })
                                return
                              }
                              enabledMutation.mutate({
                                model: m.model,
                                enabled: next,
                              })
                            }}
                            aria-label={tx("enabledAriaLabel", {
                              model: m.model,
                            })}
                          />
                        </td>
                        <td className="py-2 pr-4">
                          {/* Native radio: exactly-one selection across the
                              table, the table itself is the group. Disabled
                              for non-enabled rows so there's a visible
                              affordance you must enable it first. */}
                          <input
                            type="radio"
                            name={defaultRadioName}
                            checked={m.is_default}
                            disabled={!m.enabled || defaultMutation.isPending}
                            onChange={(e) => {
                              if (e.target.checked && !m.is_default) {
                                defaultMutation.mutate(m.model)
                              }
                            }}
                            aria-label={tx("defaultAriaLabel", {
                              model: m.model,
                            })}
                            className="size-4 cursor-pointer disabled:cursor-not-allowed disabled:opacity-40"
                          />
                        </td>
                        <td className="py-2 pr-4 font-mono text-xs">
                          {renderModelName(m)}
                          {m.is_default && (
                            <Badge variant="default" className="ml-2">
                              {tx("defaultBadge")}
                            </Badge>
                          )}
                        </td>
                        {extraColumns.map((col) => (
                          <td
                            key={col.headerKey}
                            className="py-2 pr-4 text-right font-mono"
                          >
                            {col.render(m)}
                          </td>
                        ))}
                        <td className="py-2 pr-4 text-right font-mono">
                          {speed != null
                            ? tx("speedValue", { value: Math.round(speed) })
                            : "-"}
                        </td>
                        <td className="py-2 pr-4 text-right tabular-nums">
                          {m.courses_using > 0 ? (
                            <Badge variant="outline">{m.courses_using}</Badge>
                          ) : (
                            <span className="text-muted-foreground">-</span>
                          )}
                        </td>
                        <td className="py-2 text-right">
                          <Button
                            size="sm"
                            variant="outline"
                            onClick={() => benchmarkMutation.mutate(m.model)}
                            disabled={
                              data.running || benchmarkMutation.isPending
                            }
                            title={tx("runBenchmarkTitle")}
                          >
                            {isThisRunning
                              ? tx("running")
                              : speed != null
                                ? tx("rerun")
                                : tx("run")}
                          </Button>
                        </td>
                      </tr>
                    )
                  })}
                </tbody>
              </table>
            </div>
            {(enabledMutation.isError ||
              defaultMutation.isError ||
              benchmarkMutation.isError) && (
              <p className="text-sm text-destructive">
                {formatError(
                  enabledMutation.error ??
                    defaultMutation.error ??
                    benchmarkMutation.error,
                )}
              </p>
            )}
            <p className="text-xs text-muted-foreground">{tx("note")}</p>
          </div>
        )}
      </CardContent>
      <AlertDialog
        open={confirmDisable != null}
        onOpenChange={(o) => {
          if (!o) setConfirmDisable(null)
        }}
      >
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>
              {tx("confirmDisableTitle", {
                count: confirmDisable?.coursesUsing ?? 0,
              })}
            </AlertDialogTitle>
            <AlertDialogDescription>
              {tx("confirmDisableBody", {
                model: confirmDisable?.model ?? "",
                count: confirmDisable?.coursesUsing ?? 0,
              })}
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>{tx("confirmDisableCancel")}</AlertDialogCancel>
            <AlertDialogAction
              onClick={() => {
                if (confirmDisable) {
                  enabledMutation.mutate({
                    model: confirmDisable.model,
                    enabled: false,
                  })
                  setConfirmDisable(null)
                }
              }}
            >
              {tx("confirmDisableAction")}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </Card>
  )
}
