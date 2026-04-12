import { createFileRoute } from "@tanstack/react-router"
import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query"
import { courseDocumentsQuery } from "@/lib/queries"
import { api } from "@/lib/api"
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
  const { data: documents, isLoading } = useQuery(courseDocumentsQuery(courseId))
  const queryClient = useQueryClient()
  const fileInputRef = React.useRef<HTMLInputElement>(null)
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
    mutationFn: (file: File) =>
      api.upload<DocType>(`/courses/${courseId}/documents`, file),
    onSuccess: () => {
      queryClient.invalidateQueries({
        queryKey: ["courses", courseId, "documents"],
      })
      if (fileInputRef.current) fileInputRef.current.value = ""
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
        throw new Error(`${failed} of ${docIds.length} deletes failed`)
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
        <CardTitle>Documents</CardTitle>
        <CardDescription>
          Upload PDFs and other documents for RAG
        </CardDescription>
      </CardHeader>
      <CardContent className="space-y-4">
        <div className="flex gap-2">
          <Input
            ref={fileInputRef}
            type="file"
            accept=".pdf"
            onChange={(e) => {
              const file = e.target.files?.[0]
              if (file) uploadMutation.mutate(file)
            }}
            className="flex-1"
          />
          {uploadMutation.isPending && (
            <span className="text-sm text-muted-foreground self-center">
              Uploading...
            </span>
          )}
        </div>
        {uploadMutation.isError && (
          <p className="text-sm text-destructive">
            {uploadMutation.error.message}
          </p>
        )}

        {isLoading && <p className="text-muted-foreground">Loading...</p>}

        {documents && documents.length > 0 && (
          <div className="flex items-center justify-between py-2 border-b">
            <label className="flex items-center gap-2 text-sm">
              <Checkbox
                checked={allSelected}
                indeterminate={someSelected}
                onCheckedChange={toggleAll}
              />
              <span className="text-muted-foreground">
                {selected.size > 0
                  ? `${selected.size} selected`
                  : `Select all (${documents.length})`}
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
                  ? "Deleting..."
                  : `Delete ${selected.size}`}
              </Button>
            )}
          </div>
        )}

        {bulkDeleteMutation.isError && (
          <p className="text-sm text-destructive">
            {bulkDeleteMutation.error.message}
          </p>
        )}

        <div className="space-y-2">
          {documents?.map((doc) => (
            <div
              key={doc.id}
              className="flex items-center justify-between py-2 border-b last:border-0"
            >
              <div className="flex items-center gap-3 flex-1 min-w-0">
                <Checkbox
                  checked={selected.has(doc.id)}
                  onCheckedChange={() => toggleOne(doc.id)}
                  aria-label={`Select ${doc.filename}`}
                />
                <div className="space-y-1 min-w-0">
                  <span className="font-medium truncate block">{doc.filename}</span>
                  <div className="flex gap-2 text-xs text-muted-foreground">
                    <span>{formatBytes(doc.size_bytes)}</span>
                    {doc.chunk_count != null && doc.chunk_count > 0 && (
                      <span>{doc.chunk_count} chunks</span>
                    )}
                  </div>
                </div>
              </div>
              <div className="flex items-center gap-2">
                <Badge variant={statusColor(doc.status)}>{doc.status}</Badge>
                {doc.error_msg && (
                  <span className="text-xs text-destructive" title={doc.error_msg}>
                    error
                  </span>
                )}
                <Button
                  variant={doc.displayable ? "outline" : "secondary"}
                  size="sm"
                  title={doc.displayable ? "Students can see source text" : "Source text hidden from students"}
                  onClick={() =>
                    toggleDisplayableMutation.mutate({
                      docId: doc.id,
                      displayable: !doc.displayable,
                    })
                  }
                >
                  {doc.displayable ? "Visible" : "Hidden"}
                </Button>
                <Button
                  variant="ghost"
                  size="sm"
                  onClick={() => setConfirmSingle(doc)}
                  disabled={deleteMutation.isPending}
                >
                  Delete
                </Button>
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
              <AlertDialogTitle>Delete document?</AlertDialogTitle>
              <AlertDialogDescription>
                This will permanently delete{" "}
                <span className="font-medium text-foreground">
                  {confirmSingle?.filename}
                </span>{" "}
                and remove its chunks from the vector index. This cannot be undone.
              </AlertDialogDescription>
            </AlertDialogHeader>
            <AlertDialogFooter>
              <AlertDialogCancel>Cancel</AlertDialogCancel>
              <AlertDialogAction
                variant="destructive"
                disabled={deleteMutation.isPending}
                onClick={() => {
                  if (confirmSingle) deleteMutation.mutate(confirmSingle.id)
                }}
              >
                {deleteMutation.isPending ? "Deleting..." : "Delete"}
              </AlertDialogAction>
            </AlertDialogFooter>
          </AlertDialogContent>
        </AlertDialog>

        <AlertDialog open={confirmBulk} onOpenChange={setConfirmBulk}>
          <AlertDialogContent>
            <AlertDialogHeader>
              <AlertDialogTitle>
                Delete {selected.size} document{selected.size === 1 ? "" : "s"}?
              </AlertDialogTitle>
              <AlertDialogDescription>
                This will permanently delete {selected.size} document
                {selected.size === 1 ? "" : "s"} and remove their chunks from the
                vector index. This cannot be undone.
              </AlertDialogDescription>
            </AlertDialogHeader>
            <AlertDialogFooter>
              <AlertDialogCancel>Cancel</AlertDialogCancel>
              <AlertDialogAction
                variant="destructive"
                disabled={bulkDeleteMutation.isPending}
                onClick={() => bulkDeleteMutation.mutate(Array.from(selected))}
              >
                {bulkDeleteMutation.isPending
                  ? "Deleting..."
                  : `Delete ${selected.size}`}
              </AlertDialogAction>
            </AlertDialogFooter>
          </AlertDialogContent>
        </AlertDialog>
      </CardContent>
    </Card>
  )
}
