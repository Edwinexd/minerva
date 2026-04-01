import { createFileRoute } from "@tanstack/react-router"
import { useQuery } from "@tanstack/react-query"
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
import { Skeleton } from "@/components/ui/skeleton"
import { useState } from "react"

export const Route = createFileRoute("/teacher/courses/$courseId/rag")({
  component: RagPage,
})

function RagPage() {
  const { courseId } = Route.useParams()
  return (
    <div className="space-y-4">
      <RagDebugPanel courseId={courseId} />
      <ChunkBrowser courseId={courseId} />
    </div>
  )
}

function RagDebugPanel({ courseId }: { courseId: string }) {
  const [query, setQuery] = useState("")
  const [results, setResults] = useState<
    { score: number; text: string; filename: string; chunk_index: number }[]
  >([])
  const [searching, setSearching] = useState(false)

  const doSearch = async () => {
    if (!query.trim()) return
    setSearching(true)
    try {
      const res = await api.get<typeof results>(
        `/courses/${courseId}/documents/search?q=${encodeURIComponent(query)}&limit=10`,
      )
      setResults(res)
    } catch {
      setResults([])
    } finally {
      setSearching(false)
    }
  }

  return (
    <Card>
      <CardHeader>
        <CardTitle>RAG Search</CardTitle>
        <CardDescription>
          Test semantic search against your course documents. See what chunks
          the RAG engine would retrieve for a given query.
        </CardDescription>
      </CardHeader>
      <CardContent className="space-y-4">
        <form
          className="flex gap-2"
          onSubmit={(e) => {
            e.preventDefault()
            doSearch()
          }}
        >
          <Input
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder="Enter a test query..."
            className="flex-1"
          />
          <Button type="submit" disabled={searching || !query.trim()}>
            {searching ? "Searching..." : "Search"}
          </Button>
        </form>

        {results.length > 0 && (
          <div className="space-y-3">
            {results.map((r, i) => (
              <div key={i} className="border rounded p-3 space-y-1">
                <div className="flex items-center justify-between">
                  <span className="text-sm font-medium">{r.filename}</span>
                  <Badge variant={r.score > 0.7 ? "default" : r.score > 0.5 ? "secondary" : "outline"}>
                    {r.score.toFixed(3)}
                  </Badge>
                </div>
                <p className="text-xs text-muted-foreground">
                  Chunk #{r.chunk_index}
                </p>
                <p className="text-sm whitespace-pre-wrap line-clamp-4">{r.text}</p>
              </div>
            ))}
          </div>
        )}
      </CardContent>
    </Card>
  )
}

function ChunkBrowser({ courseId }: { courseId: string }) {
  const { data: documents } = useQuery(courseDocumentsQuery(courseId))
  const [selectedDoc, setSelectedDoc] = useState<string | null>(null)
  const { data: chunks, isLoading: chunksLoading } = useQuery({
    queryKey: ["courses", courseId, "documents", selectedDoc, "chunks"],
    queryFn: () =>
      api.get<{ chunk_index: number; text: string; filename: string }[]>(
        `/courses/${courseId}/documents/${selectedDoc}/chunks`,
      ),
    enabled: !!selectedDoc,
  })

  const readyDocs = documents?.filter((d) => d.status === "ready") || []

  return (
    <Card>
      <CardHeader>
        <CardTitle>Chunk Browser</CardTitle>
        <CardDescription>
          Browse the chunks extracted from your documents
        </CardDescription>
      </CardHeader>
      <CardContent className="space-y-4">
        {readyDocs.length === 0 ? (
          <p className="text-muted-foreground text-sm">
            No processed documents yet.
          </p>
        ) : (
          <div className="flex gap-2 flex-wrap">
            {readyDocs.map((doc) => (
              <Button
                key={doc.id}
                variant={selectedDoc === doc.id ? "default" : "outline"}
                size="sm"
                onClick={() => setSelectedDoc(doc.id)}
              >
                {doc.filename} ({doc.chunk_count || 0} chunks)
              </Button>
            ))}
          </div>
        )}

        {chunksLoading && (
          <div className="space-y-2">
            <Skeleton className="h-20 w-full" />
            <Skeleton className="h-20 w-full" />
          </div>
        )}

        {chunks && (
          <div className="space-y-2 max-h-96 overflow-y-auto">
            {chunks.map((chunk) => (
              <div
                key={chunk.chunk_index}
                className="border rounded p-3 text-sm"
              >
                <div className="text-xs text-muted-foreground mb-1">
                  Chunk #{chunk.chunk_index}
                </div>
                <p className="whitespace-pre-wrap">{chunk.text}</p>
              </div>
            ))}
          </div>
        )}
      </CardContent>
    </Card>
  )
}
