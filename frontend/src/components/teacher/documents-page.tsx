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

/// Per-kind tint for the kind badge. Soft Tailwind palette so the
/// teacher gets a glanceable color signal without anything looking
/// like an error -- previously assignment_brief used the destructive
/// (red) variant and read as a system error rather than a category.
///
/// Picked to coordinate with the graph viewer's KIND_COLORS (same
/// hue family per kind) so a doc's badge here matches its node color
/// over there.
const KIND_BADGE_CLASS: Record<string, string> = {
  lecture:
    "bg-blue-100 text-blue-800 border-blue-200 dark:bg-blue-950 dark:text-blue-200 dark:border-blue-800",
  lecture_transcript:
    "bg-sky-100 text-sky-800 border-sky-200 dark:bg-sky-950 dark:text-sky-200 dark:border-sky-800",
  reading:
    "bg-emerald-100 text-emerald-800 border-emerald-200 dark:bg-emerald-950 dark:text-emerald-200 dark:border-emerald-800",
  tutorial_exercise:
    "bg-teal-100 text-teal-800 border-teal-200 dark:bg-teal-950 dark:text-teal-200 dark:border-teal-800",
  // assignment_brief / lab_brief / exam: warm but not destructive.
  // Teachers should *notice* assessment kinds (chat path treats them
  // specially) without reading them as errors.
  assignment_brief:
    "bg-amber-100 text-amber-900 border-amber-200 dark:bg-amber-950 dark:text-amber-100 dark:border-amber-800",
  lab_brief:
    "bg-orange-100 text-orange-900 border-orange-200 dark:bg-orange-950 dark:text-orange-100 dark:border-orange-800",
  exam:
    "bg-rose-100 text-rose-900 border-rose-200 dark:bg-rose-950 dark:text-rose-100 dark:border-rose-800",
  sample_solution:
    "bg-violet-100 text-violet-900 border-violet-200 dark:bg-violet-950 dark:text-violet-100 dark:border-violet-800",
  syllabus:
    "bg-slate-100 text-slate-800 border-slate-200 dark:bg-slate-900 dark:text-slate-200 dark:border-slate-700",
  unknown:
    "bg-zinc-100 text-zinc-700 border-zinc-200 dark:bg-zinc-900 dark:text-zinc-300 dark:border-zinc-700",
}

const UNCLASSIFIED_BADGE_CLASS =
  "bg-muted text-muted-foreground border-dashed"

function kindBadgeClass(kind: DocumentKind | null): string {
  if (kind == null) return UNCLASSIFIED_BADGE_CLASS
  return KIND_BADGE_CLASS[kind] ?? UNCLASSIFIED_BADGE_CLASS
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

  // Bulk reclassify the currently-selected documents. We loop the
  // existing per-doc endpoint rather than introducing a new bulk
  // endpoint -- it's the same code path the dialog's "Re-classify"
  // button uses, so behaviour stays consistent (locked rows skip,
  // failures are reported individually). Each docId is fired in
  // parallel with bounded concurrency via Promise.allSettled, then
  // we surface a summary on completion.
  const bulkReclassifyMutation = useMutation({
    mutationFn: async (docIds: string[]) => {
      const docsById = new Map(documents?.map((d) => [d.id, d]) ?? [])
      // Drop locked rows up front -- the per-doc endpoint silently
      // returns `{locked: true}` for them, which we'd otherwise count
      // as a success and confuse the teacher.
      const eligible = docIds.filter(
        (id) => !docsById.get(id)?.kind_locked_by_teacher,
      )
      const skippedLocked = docIds.length - eligible.length
      const results = await Promise.allSettled(
        eligible.map((id) =>
          api.post(`/courses/${courseId}/documents/${id}/reclassify`, {}),
        ),
      )
      const failed = results.filter((r) => r.status === "rejected").length
      if (failed > 0) {
        throw new Error(
          t("documents.reclassifySelectedFailed", {
            failed,
            total: eligible.length,
          }),
        )
      }
      return { ok: eligible.length, skippedLocked }
    },
    onSettled: () => {
      queryClient.invalidateQueries({
        queryKey: ["courses", courseId, "documents"],
      })
    },
    onSuccess: () => {
      setSelected(new Set())
    },
  })

  const statusColor = (status: string) => {
    if (status === "ready") return "default" as const
    if (status === "processing") return "secondary" as const
    if (status === "failed") return "destructive" as const
    return "outline" as const
  }

  // Visual signal: each kind gets a soft semantic tint via
  // KIND_BADGE_CLASS rather than the destructive (red) variant we
  // used to use for assessment kinds. The destructive look read as a
  // system error; the new palette keeps assessments distinct (warm
  // amber/orange/rose) while staying calm.

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
              {selected.size > 0 && (
                <>
                  <Button
                    variant="outline"
                    size="sm"
                    title={t("documents.reclassifySelectedTitle")}
                    onClick={() =>
                      bulkReclassifyMutation.mutate(Array.from(selected))
                    }
                    disabled={bulkReclassifyMutation.isPending}
                  >
                    {bulkReclassifyMutation.isPending
                      ? t("documents.reclassifyingSelected")
                      : t("documents.reclassifySelected", {
                          count: selected.size,
                        })}
                  </Button>
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
                </>
              )}
            </div>
          </div>
        )}

        {bulkDeleteMutation.isError && (
          <p className="text-sm text-destructive">
            {formatError(bulkDeleteMutation.error)}
          </p>
        )}
        {bulkReclassifyMutation.isError && (
          <p className="text-sm text-destructive">
            {formatError(bulkReclassifyMutation.error)}
          </p>
        )}
        {bulkReclassifyMutation.isSuccess &&
          bulkReclassifyMutation.data &&
          bulkReclassifyMutation.data.skippedLocked > 0 && (
            <p className="text-sm text-muted-foreground">
              {t("documents.reclassifyLockedSkipped", {
                count: bulkReclassifyMutation.data.skippedLocked,
              })}
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
                  variant="outline"
                  className={`${kindBadgeClass(doc.kind)} ${canMutate ? "cursor-pointer hover:opacity-80" : ""}`}
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
