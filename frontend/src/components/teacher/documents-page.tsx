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
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select"
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
import type { Document as DocType, DocumentKind } from "@/lib/types"
import { DOCUMENT_KINDS } from "@/lib/types"

function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`
}

/// Multi-line tooltip surfaced by hovering the kind badge: lock
/// state, classifier confidence, and rationale. Browsers render
/// `\n` in `title=` as a soft line break which is good enough for a
/// hover affordance without bringing in a Tooltip primitive.
function kindBadgeTooltip(
  doc: DocType,
  t: (key: string, opts?: Record<string, unknown>) => string,
): string {
  const lines: string[] = []
  if (doc.kind) {
    lines.push(t(`documents.kindLabel.${doc.kind}`))
  } else {
    lines.push(t("documents.kindLabel.unclassified"))
  }
  if (doc.kind_locked_by_teacher) {
    lines.push(t("documents.kindLockedSubtext"))
  } else if (doc.kind_confidence != null) {
    lines.push(
      t("documents.kindConfidenceSubtext", {
        pct: Math.round(doc.kind_confidence * 100),
      }),
    )
  }
  if (doc.kind_rationale) {
    lines.push(doc.kind_rationale)
  }
  return lines.join("\n")
}

export function DocumentsPage({ useParams }: { useParams: () => { courseId: string } }) {
  const { courseId } = useParams()
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
  // Edit-kind dialog state. Holds the doc being edited; the dialog
  // owns its own pending-kind selection so the row's badge keeps
  // showing the current value until the teacher confirms.
  const [editingKind, setEditingKind] = React.useState<DocType | null>(null)

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

  const setKindMutation = useMutation({
    mutationFn: ({ docId, kind }: { docId: string; kind: DocumentKind }) =>
      api.patch(`/courses/${courseId}/documents/${docId}/kind`, { kind }),
    onSuccess: () => {
      queryClient.invalidateQueries({
        queryKey: ["courses", courseId, "documents"],
      })
    },
  })

  const clearLockMutation = useMutation({
    mutationFn: ({ docId }: { docId: string }) =>
      api.delete(`/courses/${courseId}/documents/${docId}/kind/lock`),
    onSuccess: () => {
      queryClient.invalidateQueries({
        queryKey: ["courses", courseId, "documents"],
      })
    },
  })

  const reclassifyMutation = useMutation({
    mutationFn: ({ docId }: { docId: string }) =>
      api.post(`/courses/${courseId}/documents/${docId}/reclassify`, {}),
    onSuccess: () => {
      queryClient.invalidateQueries({
        queryKey: ["courses", courseId, "documents"],
      })
    },
  })

  const reclassifyAllMutation = useMutation({
    mutationFn: () =>
      api.post(`/courses/${courseId}/documents/reclassify-all`, {}),
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

  // Visual signal: assignment-bearing kinds (which the chat path
  // refuses to solve from) get a destructive badge so the teacher can
  // spot them at a glance; sample_solution gets the secondary/muted
  // look since it's locked out of RAG entirely. Lecture/reading/
  // syllabus are the "normal" surface.
  const kindBadgeVariant = (kind: DocumentKind | null) => {
    if (kind == null) return "outline" as const
    if (
      kind === "assignment_brief" ||
      kind === "lab_brief" ||
      kind === "exam"
    ) {
      return "destructive" as const
    }
    if (kind === "sample_solution") return "secondary" as const
    return "default" as const
  }

  // When the kind dialog closes after a successful mutation, sync the
  // displayed editingKind state with the freshly-fetched row so a
  // teacher who chains operations (e.g. unlock then re-classify) sees
  // the latest state without closing/reopening.
  const editingKindFresh = React.useMemo(() => {
    if (!editingKind || !documents) return editingKind
    return documents.find((d) => d.id === editingKind.id) ?? editingKind
  }, [editingKind, documents])

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
            <div className="flex items-center gap-2">
              <Button
                variant="outline"
                size="sm"
                title={t("documents.reclassifyAllTitle")}
                onClick={() => reclassifyAllMutation.mutate()}
                disabled={reclassifyAllMutation.isPending}
              >
                {reclassifyAllMutation.isPending
                  ? t("documents.reclassifyingAll")
                  : t("documents.reclassifyAll")}
              </Button>
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
                {/*
                  Single clickable kind badge -- opens the edit dialog
                  where the teacher can override, re-classify, or
                  unlock. Tooltip surfaces rationale + confidence so
                  there's still a hover-to-inspect path without
                  opening the dialog. When the row can't be mutated
                  (TA, etc.) the badge stays purely informational.
                */}
                <Badge
                  variant={doc.kind ? kindBadgeVariant(doc.kind) : "outline"}
                  className={canMutate ? "cursor-pointer hover:opacity-80" : ""}
                  onClick={canMutate ? () => setEditingKind(doc) : undefined}
                  title={kindBadgeTooltip(doc, t)}
                  aria-label={
                    canMutate
                      ? t("documents.setKindAria", { filename: doc.filename })
                      : undefined
                  }
                >
                  {doc.kind
                    ? t(`documents.kindLabel.${doc.kind}`)
                    : t("documents.kindLabel.unclassified")}
                  {doc.kind_locked_by_teacher && (
                    <span className="ml-1 opacity-70" aria-hidden>
                      {"\u{1F512}"}
                    </span>
                  )}
                </Badge>
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

        {/*
          Edit-kind dialog. The badge in the row is the trigger; this
          dialog is the only place the teacher can: pick a manual
          override (Select), trigger a fresh classification, or clear
          a teacher lock. Kept separate from the row so the row stays
          a clean two-badge / two-button layout.
        */}
        <AlertDialog
          open={editingKind !== null}
          onOpenChange={(open) => {
            if (!open) setEditingKind(null)
          }}
        >
          <AlertDialogContent>
            <AlertDialogHeader>
              <AlertDialogTitle>{t("documents.kindDialogTitle")}</AlertDialogTitle>
              <AlertDialogDescription>
                {editingKindFresh?.filename}
              </AlertDialogDescription>
            </AlertDialogHeader>

            {editingKindFresh && (
              <div className="space-y-1">
                <div className="text-xs text-muted-foreground">
                  {editingKindFresh.kind_locked_by_teacher
                    ? t("documents.kindLockedSubtext")
                    : editingKindFresh.kind_confidence != null
                      ? t("documents.kindConfidenceSubtext", {
                          pct: Math.round(
                            editingKindFresh.kind_confidence * 100,
                          ),
                        })
                      : t("documents.kindLabel.unclassified")}
                </div>
                {editingKindFresh.kind_rationale && (
                  <div className="text-xs italic text-muted-foreground">
                    {editingKindFresh.kind_rationale}
                  </div>
                )}
              </div>
            )}

            <div className="space-y-4 py-2">
              <div className="space-y-1">
                <label className="text-sm font-medium">
                  {t("documents.kindDialogOverrideLabel")}
                </label>
                <Select
                  value={editingKindFresh?.kind ?? ""}
                  onValueChange={(value) => {
                    if (!value || !editingKindFresh) return
                    setKindMutation.mutate({
                      docId: editingKindFresh.id,
                      kind: value as DocumentKind,
                    })
                  }}
                  disabled={setKindMutation.isPending}
                >
                  <SelectTrigger className="w-full">
                    <SelectValue
                      placeholder={t("documents.setKindPlaceholder")}
                    />
                  </SelectTrigger>
                  <SelectContent>
                    {DOCUMENT_KINDS.map((k) => (
                      <SelectItem key={k} value={k}>
                        {t(`documents.kindLabel.${k}`)}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
                <p className="text-xs text-muted-foreground">
                  {t("documents.kindDialogOverrideHelp")}
                </p>
              </div>

              <div className="flex flex-wrap gap-2">
                {editingKindFresh?.kind_locked_by_teacher ? (
                  <Button
                    variant="outline"
                    size="sm"
                    onClick={() => {
                      if (editingKindFresh) {
                        clearLockMutation.mutate({ docId: editingKindFresh.id })
                      }
                    }}
                    disabled={clearLockMutation.isPending}
                  >
                    {t("documents.clearLock")}
                  </Button>
                ) : (
                  editingKindFresh?.status === "ready" && (
                    <Button
                      variant="outline"
                      size="sm"
                      onClick={() => {
                        if (editingKindFresh) {
                          reclassifyMutation.mutate({
                            docId: editingKindFresh.id,
                          })
                        }
                      }}
                      disabled={reclassifyMutation.isPending}
                    >
                      {reclassifyMutation.isPending
                        ? t("documents.reclassifying")
                        : t("documents.reclassify")}
                    </Button>
                  )
                )}
              </div>
            </div>

            <AlertDialogFooter>
              <AlertDialogCancel>{tCommon("actions.close")}</AlertDialogCancel>
            </AlertDialogFooter>
          </AlertDialogContent>
        </AlertDialog>
      </CardContent>
    </Card>
  )
}
