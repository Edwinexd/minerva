import { createFileRoute } from "@tanstack/react-router"
import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query"
import { useTranslation } from "react-i18next"
import { courseDocumentsQuery, courseQuery } from "@/lib/queries"
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
import { Badge } from "@/components/ui/badge"
import { Checkbox } from "@/components/ui/checkbox"
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
import React from "react"
import type { Document as DocType } from "@/lib/types"

export const Route = createFileRoute("/teacher/courses/$courseId/documents")({
  component: DocumentsPage,
})

function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`
}

function DocumentsPage() {
  const { courseId } = Route.useParams()
  const { t } = useTranslation("teacher")
  const { t: tCommon } = useTranslation("common")
  const formatError = useApiErrorMessage()
  const { data: documents, isLoading } = useQuery(courseDocumentsQuery(courseId))
  const { data: course } = useQuery(courseQuery(courseId))
  const canMutate = course?.my_role !== "ta"
  const queryClient = useQueryClient()
  const fileInputRef = React.useRef<HTMLInputElement>(null)
  const mbzInputRef = React.useRef<HTMLInputElement>(null)
  const [mbzResult, setMbzResult] = React.useState<{
    imported: number
    skippedHidden: number
  } | null>(null)
  const [selected, setSelected] = React.useState<Set<string>>(new Set())
  const [confirmSingle, setConfirmSingle] = React.useState<DocType | null>(null)
  const [confirmBulk, setConfirmBulk] = React.useState(false)

  // Drop selections that no longer exist (e.g. after a delete round-trip).
  React.useEffect(() => {
    if (!documents) return
    const existing = new Set(documents.map((d) => d.id))
    setSelected((prev) => {
      const next = new Set<string>()
      for (const id of prev) if (existing.has(id)) next.add(id)
      return next.size === prev.size ? prev : next
    })
  }, [documents])

  const uploadMutation = useMutation({
    mutationFn: async (files: File[]) => {
      const results = await Promise.allSettled(
        files.map((file) =>
          api.upload<DocType>(`/courses/${courseId}/documents`, file),
        ),
      )
      const failed = results.filter((r) => r.status === "rejected")
      if (failed.length > 0) {
        const messages = failed
          .map((r) => {
            const reason = (r as PromiseRejectedResult).reason
            return reason ? formatError(reason) : t("documents.unknownError")
          })
          .join(", ")
        throw new Error(t("documents.uploadFailed", { failed: failed.length, total: files.length, messages }))
      }
    },
    onSuccess: () => {
      queryClient.invalidateQueries({
        queryKey: ["courses", courseId, "documents"],
      })
      if (fileInputRef.current) fileInputRef.current.value = ""
    },
  })

  const mbzMutation = useMutation({
    mutationFn: (file: File) =>
      api.upload<{ imported: number; skipped_hidden: number }>(
        `/courses/${courseId}/documents/mbz`,
        file,
      ),
    onSuccess: (res) => {
      setMbzResult({ imported: res.imported, skippedHidden: res.skipped_hidden })
      queryClient.invalidateQueries({
        queryKey: ["courses", courseId, "documents"],
      })
      if (mbzInputRef.current) mbzInputRef.current.value = ""
    },
  })

  const deleteMutation = useMutation({
    mutationFn: (docId: string) =>
      api.delete(`/courses/${courseId}/documents/${docId}`),
    onSuccess: () => {
      setConfirmSingle(null)
      queryClient.invalidateQueries({
        queryKey: ["courses", courseId, "documents"],
      })
    },
  })

  const bulkDeleteMutation = useMutation({
    mutationFn: async (docIds: string[]) => {
      const results = await Promise.allSettled(
        docIds.map((id) =>
          api.delete(`/courses/${courseId}/documents/${id}`),
        ),
      )
      const failed = results.filter((r) => r.status === "rejected").length
      if (failed > 0) {
        throw new Error(t("documents.bulkDeleteFailed", { failed, total: docIds.length }))
      }
      return { deleted: docIds.length }
    },
    onSettled: () => {
      queryClient.invalidateQueries({
        queryKey: ["courses", courseId, "documents"],
      })
    },
    onSuccess: () => {
      setSelected(new Set())
      setConfirmBulk(false)
    },
  })

  const toggleOne = (id: string) =>
    setSelected((prev) => {
      const next = new Set(prev)
      if (next.has(id)) next.delete(id)
      else next.add(id)
      return next
    })

  const allSelected =
    !!documents && documents.length > 0 && selected.size === documents.length
  const someSelected = selected.size > 0 && !allSelected

  const toggleAll = () => {
    if (!documents) return
    if (allSelected) {
      setSelected(new Set())
    } else {
      setSelected(new Set(documents.map((d) => d.id)))
    }
  }

  const toggleDisplayableMutation = useMutation({
    mutationFn: ({ docId, displayable }: { docId: string; displayable: boolean }) =>
      api.patch(`/courses/${courseId}/documents/${docId}`, { displayable }),
    onSuccess: () => {
      queryClient.invalidateQueries({
        queryKey: ["courses", courseId, "documents"],
      })
    },
  })

  const statusColor = (status: string) => {
    if (status === "ready") return "default" as const
    if (status === "processing") return "secondary" as const
    if (status === "failed") return "destructive" as const
    return "outline" as const
  }

  return (
    <Card>
      <CardHeader>
        <CardTitle>{t("documents.title")}</CardTitle>
        <CardDescription>
          {t("documents.description")}
        </CardDescription>
      </CardHeader>
      <CardContent className="space-y-4">
        {canMutate && (
          <>
            <div className="flex gap-2">
              <Input
                ref={fileInputRef}
                type="file"
                accept=".pdf"
                multiple
                onChange={(e) => {
                  const files = Array.from(e.target.files ?? [])
                  if (files.length > 0) uploadMutation.mutate(files)
                }}
                className="flex-1"
              />
              {uploadMutation.isPending && (
                <span className="text-sm text-muted-foreground self-center">
                  {t("documents.uploading")}
                </span>
              )}
            </div>
            {uploadMutation.isError && (
              <p className="text-sm text-destructive">
                {formatError(uploadMutation.error)}
              </p>
            )}

            <div className="space-y-1">
              <p className="text-sm font-medium">
                {t("documents.mbzTitle")}
              </p>
              <p className="text-xs text-muted-foreground">
                {t("documents.mbzDescription")}
              </p>
              <div className="flex gap-2">
                <Input
                  ref={mbzInputRef}
                  type="file"
                  accept=".mbz"
                  onChange={(e) => {
                    const file = e.target.files?.[0]
                    if (file) {
                      setMbzResult(null)
                      mbzMutation.mutate(file)
                    }
                  }}
                  className="flex-1"
                  disabled={mbzMutation.isPending}
                />
                {mbzMutation.isPending && (
                  <span className="text-sm text-muted-foreground self-center">
                    {t("documents.mbzImporting")}
                  </span>
                )}
              </div>
              {mbzMutation.isError && (
                <p className="text-sm text-destructive">
                  {formatError(mbzMutation.error)}
                </p>
              )}
              {mbzResult && (
                <p className="text-sm text-muted-foreground">
                  {t("documents.mbzResult", {
                    imported: mbzResult.imported,
                    skipped: mbzResult.skippedHidden,
                  })}
                </p>
              )}
            </div>
          </>
        )}

        {isLoading && <p className="text-muted-foreground">{tCommon("status.loading")}</p>}

        {canMutate && documents && documents.length > 0 && (
          <div className="flex items-center justify-between py-2 border-b">
            <label className="flex items-center gap-2 text-sm">
              <Checkbox
                checked={allSelected}
                indeterminate={someSelected}
                onCheckedChange={toggleAll}
              />
              <span className="text-muted-foreground">
                {selected.size > 0
                  ? t("documents.selectedCount", { count: selected.size })
                  : t("documents.selectAll", { count: documents.length })}
              </span>
            </label>
            {selected.size > 0 && (
              <Button
                variant="destructive"
                size="sm"
                onClick={() => setConfirmBulk(true)}
                disabled={bulkDeleteMutation.isPending}
              >
                {bulkDeleteMutation.isPending
                  ? t("documents.deletingCount")
                  : t("documents.deleteCount", { count: selected.size })}
              </Button>
            )}
          </div>
        )}

        {bulkDeleteMutation.isError && (
          <p className="text-sm text-destructive">
            {formatError(bulkDeleteMutation.error)}
          </p>
        )}

        <div className="space-y-2">
          {documents?.map((doc) => (
            <div
              key={doc.id}
              className="flex items-center justify-between py-2 border-b last:border-0"
            >
              <div className="flex items-center gap-3 flex-1 min-w-0">
                {canMutate && (
                  <Checkbox
                    checked={selected.has(doc.id)}
                    onCheckedChange={() => toggleOne(doc.id)}
                    aria-label={t("documents.selectDocAria", { filename: doc.filename })}
                  />
                )}
                <div className="space-y-1 min-w-0">
                  <span className="font-medium truncate block">{doc.filename}</span>
                  <div className="flex gap-2 text-xs text-muted-foreground">
                    <span>{formatBytes(doc.size_bytes)}</span>
                    {doc.chunk_count != null && doc.chunk_count > 0 && (
                      <span>{t("documents.chunksSuffix", { count: doc.chunk_count })}</span>
                    )}
                  </div>
                </div>
              </div>
              <div className="flex items-center gap-2">
                <Badge variant={statusColor(doc.status)}>{doc.status}</Badge>
                {doc.error_msg && (
                  <span className="text-xs text-destructive" title={doc.error_msg}>
                    {t("documents.errorLabel")}
                  </span>
                )}
                {canMutate ? (
                  <Button
                    variant={doc.displayable ? "outline" : "secondary"}
                    size="sm"
                    title={doc.displayable ? t("documents.visibleTitle") : t("documents.hiddenTitle")}
                    onClick={() =>
                      toggleDisplayableMutation.mutate({
                        docId: doc.id,
                        displayable: !doc.displayable,
                      })
                    }
                  >
                    {doc.displayable ? t("documents.visible") : t("documents.hidden")}
                  </Button>
                ) : (
                  <Badge variant="outline">{doc.displayable ? t("documents.visible") : t("documents.hidden")}</Badge>
                )}
                {canMutate && (
                  <Button
                    variant="ghost"
                    size="sm"
                    onClick={() => setConfirmSingle(doc)}
                    disabled={deleteMutation.isPending}
                  >
                    {t("documents.deleteOne")}
                  </Button>
                )}
              </div>
            </div>
          ))}
        </div>

        <AlertDialog
          open={confirmSingle !== null}
          onOpenChange={(open) => {
            if (!open) setConfirmSingle(null)
          }}
        >
          <AlertDialogContent>
            <AlertDialogHeader>
              <AlertDialogTitle>{t("documents.confirmSingleTitle")}</AlertDialogTitle>
              <AlertDialogDescription>
                {t("documents.confirmSingleBody1")}{" "}
                <span className="font-medium text-foreground">
                  {confirmSingle?.filename}
                </span>{" "}
                {t("documents.confirmSingleBody2")}
              </AlertDialogDescription>
            </AlertDialogHeader>
            <AlertDialogFooter>
              <AlertDialogCancel>{tCommon("actions.cancel")}</AlertDialogCancel>
              <AlertDialogAction
                variant="destructive"
                disabled={deleteMutation.isPending}
                onClick={() => {
                  if (confirmSingle) deleteMutation.mutate(confirmSingle.id)
                }}
              >
                {deleteMutation.isPending ? t("documents.deleting") : tCommon("actions.delete")}
              </AlertDialogAction>
            </AlertDialogFooter>
          </AlertDialogContent>
        </AlertDialog>

        <AlertDialog open={confirmBulk} onOpenChange={setConfirmBulk}>
          <AlertDialogContent>
            <AlertDialogHeader>
              <AlertDialogTitle>
                {t("documents.confirmBulkTitle", { count: selected.size })}
              </AlertDialogTitle>
              <AlertDialogDescription>
                {t("documents.confirmBulkBody", { count: selected.size })}
              </AlertDialogDescription>
            </AlertDialogHeader>
            <AlertDialogFooter>
              <AlertDialogCancel>{tCommon("actions.cancel")}</AlertDialogCancel>
              <AlertDialogAction
                variant="destructive"
                disabled={bulkDeleteMutation.isPending}
                onClick={() => bulkDeleteMutation.mutate(Array.from(selected))}
              >
                {bulkDeleteMutation.isPending
                  ? t("documents.deletingCount")
                  : t("documents.deleteCount", { count: selected.size })}
              </AlertDialogAction>
            </AlertDialogFooter>
          </AlertDialogContent>
        </AlertDialog>
      </CardContent>
    </Card>
  )
}
