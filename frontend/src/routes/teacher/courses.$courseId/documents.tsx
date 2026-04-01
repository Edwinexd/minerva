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
      queryClient.invalidateQueries({
        queryKey: ["courses", courseId, "documents"],
      })
    },
  })

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

        <div className="space-y-2">
          {documents?.map((doc) => (
            <div
              key={doc.id}
              className="flex items-center justify-between py-2 border-b last:border-0"
            >
              <div className="space-y-1">
                <span className="font-medium">{doc.filename}</span>
                <div className="flex gap-2 text-xs text-muted-foreground">
                  <span>{formatBytes(doc.size_bytes)}</span>
                  {doc.chunk_count != null && doc.chunk_count > 0 && (
                    <span>{doc.chunk_count} chunks</span>
                  )}
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
                  onClick={() => deleteMutation.mutate(doc.id)}
                >
                  Delete
                </Button>
              </div>
            </div>
          ))}
        </div>
      </CardContent>
    </Card>
  )
}
