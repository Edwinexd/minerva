import { createFileRoute, Link } from "@tanstack/react-router"
import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query"
import { courseQuery, courseMembersQuery, courseDocumentsQuery, modelsQuery, allConversationsQuery, conversationDetailQuery } from "@/lib/queries"
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
import { Label } from "@/components/ui/label"
import { Slider } from "@/components/ui/slider"
import { Textarea } from "@/components/ui/textarea"
import { Badge } from "@/components/ui/badge"
import { Separator } from "@/components/ui/separator"
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs"
import { Skeleton } from "@/components/ui/skeleton"
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select"
import Markdown from "react-markdown"
import remarkGfm from "remark-gfm"
import React, { useState } from "react"
import type { ConversationWithUser, Course, Document as DocType, TeacherNote } from "@/lib/types"

export const Route = createFileRoute("/teacher/courses/$courseId")({
  component: CourseEditPage,
})

function CourseEditPage() {
  const { courseId } = Route.useParams()
  const { data: course, isLoading } = useQuery(courseQuery(courseId))

  if (isLoading) return (
    <div className="space-y-6">
      <Skeleton className="h-8 w-64" />
      <Skeleton className="h-10 w-80" />
      <Skeleton className="h-64 w-full" />
    </div>
  )
  if (!course) return <p className="text-muted-foreground">Course not found</p>

  return (
    <div className="space-y-6">
      <div className="flex items-center justify-between">
        <h2 className="text-2xl font-bold tracking-tight">{course.name}</h2>
        <Link to="/course/$courseId" params={{ courseId }}>
          <Button variant="outline">Try Chat</Button>
        </Link>
      </div>

      <Tabs defaultValue="config">
        <TabsList>
          <TabsTrigger value="config">Configuration</TabsTrigger>
          <TabsTrigger value="members">Members</TabsTrigger>
          <TabsTrigger value="conversations">Conversations</TabsTrigger>
          <TabsTrigger value="documents">Documents</TabsTrigger>
          <TabsTrigger value="invite">Invite Links</TabsTrigger>
          <TabsTrigger value="rag">RAG Debug</TabsTrigger>
        </TabsList>

        <TabsContent value="config" className="mt-4">
          <CourseConfigForm course={course} />
        </TabsContent>

        <TabsContent value="members" className="mt-4">
          <MembersPanel courseId={courseId} />
        </TabsContent>

        <TabsContent value="conversations" className="mt-4">
          <ConversationsPanel courseId={courseId} />
        </TabsContent>

        <TabsContent value="documents" className="mt-4">
          <DocumentsPanel courseId={courseId} />
        </TabsContent>

        <TabsContent value="invite" className="mt-4">
          <InviteLinksPanel courseId={courseId} />
        </TabsContent>

        <TabsContent value="rag" className="mt-4">
          <RagDebugPanel courseId={courseId} />
        </TabsContent>
      </Tabs>
    </div>
  )
}

function CourseConfigForm({ course }: { course: Course }) {
  const queryClient = useQueryClient()
  const { data: modelsData } = useQuery(modelsQuery)
  const [name, setName] = useState(course.name)
  const [description, setDescription] = useState(course.description || "")
  const [contextRatio, setContextRatio] = useState(course.context_ratio)
  const [temperature, setTemperature] = useState(course.temperature)
  const [model, setModel] = useState(course.model)
  const [systemPrompt, setSystemPrompt] = useState(course.system_prompt || "")
  const [maxChunks, setMaxChunks] = useState(course.max_chunks)
  const [strategy, setStrategy] = useState(course.strategy)

  const mutation = useMutation({
    mutationFn: (data: Record<string, unknown>) =>
      api.put<Course>(`/courses/${course.id}`, data),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["courses"] })
    },
  })

  return (
    <Card>
      <CardHeader>
        <CardTitle>Course Configuration</CardTitle>
        <CardDescription>
          Configure how RAG works for this course
        </CardDescription>
      </CardHeader>
      <CardContent>
        <form
          className="space-y-6"
          onSubmit={(e) => {
            e.preventDefault()
            mutation.mutate({
              name,
              description: description || null,
              context_ratio: contextRatio,
              temperature,
              model,
              system_prompt: systemPrompt || null,
              max_chunks: maxChunks,
              strategy,
            })
          }}
        >
          <div className="space-y-2">
            <Label htmlFor="name">Course Name</Label>
            <Input
              id="name"
              value={name}
              onChange={(e) => setName(e.target.value)}
            />
          </div>

          <div className="space-y-2">
            <Label htmlFor="description">Description</Label>
            <Textarea
              id="description"
              value={description}
              onChange={(e) => setDescription(e.target.value)}
            />
          </div>

          <Separator />

          <div className="space-y-2">
            <Label>
              RAG Context Ratio: {Math.round(contextRatio * 100)}%
            </Label>
            <Slider
              value={[contextRatio]}
              onValueChange={(v) => setContextRatio(Array.isArray(v) ? v[0] : v)}
              min={0.1}
              max={0.95}
              step={0.05}
            />
            <p className="text-xs text-muted-foreground">
              How much of the context window is used for RAG chunks vs
              conversation history
            </p>
          </div>

          <div className="space-y-2">
            <Label>Temperature: {temperature.toFixed(2)}</Label>
            <Slider
              value={[temperature]}
              onValueChange={(v) => setTemperature(Array.isArray(v) ? v[0] : v)}
              min={0}
              max={1}
              step={0.05}
            />
          </div>

          <div className="space-y-2">
            <Label>Model</Label>
            <Select value={model} onValueChange={(v) => v && setModel(v)}>
              <SelectTrigger>
                <SelectValue placeholder="Select a model" />
              </SelectTrigger>
              <SelectContent>
                {modelsData?.models.map((m) => (
                  <SelectItem key={m.id} value={m.id}>
                    {m.name}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </div>

          <div className="space-y-2">
            <Label>Generation Strategy</Label>
            <Select value={strategy} onValueChange={(v) => v && setStrategy(v)}>
              <SelectTrigger>
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="parallel">
                  Parallel (fast first token, inject RAG mid-stream)
                </SelectItem>
                <SelectItem value="simple">
                  Simple (retrieve first, then generate)
                </SelectItem>
                <SelectItem value="flare">
                  FLARE (sentence-level active retrieval)
                </SelectItem>
              </SelectContent>
            </Select>
            <p className="text-xs text-muted-foreground">
              Parallel starts streaming instantly and injects RAG context when ready.
              Simple waits for RAG before generating. FLARE retrieves after each sentence
              using the generated text as the query.
            </p>
          </div>

          <div className="space-y-2">
            <Label htmlFor="maxChunks">Max Retrieved Chunks</Label>
            <Input
              id="maxChunks"
              type="number"
              value={maxChunks}
              onChange={(e) => setMaxChunks(parseInt(e.target.value) || 10)}
              min={1}
              max={50}
            />
          </div>

          <div className="space-y-2">
            <Label htmlFor="systemPrompt">Custom System Prompt</Label>
            <Textarea
              id="systemPrompt"
              value={systemPrompt}
              onChange={(e) => setSystemPrompt(e.target.value)}
              placeholder="Optional custom instructions for the AI assistant"
              rows={4}
            />
          </div>

          <Button type="submit" disabled={mutation.isPending}>
            {mutation.isPending ? "Saving..." : "Save Configuration"}
          </Button>
          {mutation.isSuccess && (
            <span className="text-sm text-muted-foreground ml-2">Saved!</span>
          )}
          {mutation.isError && (
            <p className="text-sm text-destructive">{mutation.error.message}</p>
          )}
        </form>
      </CardContent>
    </Card>
  )
}

function MembersPanel({ courseId }: { courseId: string }) {
  const { data: members, isLoading } = useQuery(courseMembersQuery(courseId))
  const queryClient = useQueryClient()
  const [eppn, setEppn] = useState("")
  const [role, setRole] = useState("student")

  const addMutation = useMutation({
    mutationFn: (data: { eppn: string; role: string }) =>
      api.post(`/courses/${courseId}/members`, data),
    onSuccess: () => {
      queryClient.invalidateQueries({
        queryKey: ["courses", courseId, "members"],
      })
      setEppn("")
    },
  })

  const removeMutation = useMutation({
    mutationFn: (userId: string) =>
      api.delete(`/courses/${courseId}/members/${userId}`),
    onSuccess: () => {
      queryClient.invalidateQueries({
        queryKey: ["courses", courseId, "members"],
      })
    },
  })

  return (
    <Card>
      <CardHeader>
        <CardTitle>Members</CardTitle>
        <CardDescription>Manage who has access to this course</CardDescription>
      </CardHeader>
      <CardContent className="space-y-4">
        <form
          className="flex gap-2"
          onSubmit={(e) => {
            e.preventDefault()
            if (eppn) addMutation.mutate({ eppn, role })
          }}
        >
          <Input
            value={eppn}
            onChange={(e) => setEppn(e.target.value)}
            placeholder="username@SU.SE"
            className="flex-1"
          />
          <select
            value={role}
            onChange={(e) => setRole(e.target.value)}
            className="border rounded px-2 text-sm"
          >
            <option value="student">Student</option>
            <option value="ta">TA</option>
            <option value="teacher">Teacher</option>
          </select>
          <Button type="submit" disabled={addMutation.isPending}>
            Add
          </Button>
        </form>

        {isLoading && <p className="text-muted-foreground">Loading...</p>}

        <div className="space-y-2">
          {members?.map((m) => (
            <div
              key={m.user_id}
              className="flex items-center justify-between py-2 border-b last:border-0"
            >
              <div>
                <span className="font-medium">
                  {m.display_name || m.eppn}
                </span>
                {m.display_name && (
                  <span className="text-muted-foreground text-sm ml-2">
                    {m.eppn}
                  </span>
                )}
              </div>
              <div className="flex items-center gap-2">
                <Badge variant="outline">{m.role}</Badge>
                <Button
                  variant="ghost"
                  size="sm"
                  onClick={() => removeMutation.mutate(m.user_id)}
                >
                  Remove
                </Button>
              </div>
            </div>
          ))}
        </div>
      </CardContent>
    </Card>
  )
}

function DocumentsPanel({ courseId }: { courseId: string }) {
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

function InviteLinksPanel({ courseId }: { courseId: string }) {
  const queryClient = useQueryClient()
  const { data: links, isLoading } = useQuery({
    queryKey: ["courses", courseId, "signed-urls"],
    queryFn: () =>
      api.get<
        {
          id: string
          token: string
          url: string
          expires_at: string
          max_uses: number | null
          use_count: number
        }[]
      >(`/courses/${courseId}/signed-urls`),
  })

  const createMutation = useMutation({
    mutationFn: (data: { expires_in_hours?: number; max_uses?: number }) =>
      api.post(`/courses/${courseId}/signed-urls`, data),
    onSuccess: () => {
      queryClient.invalidateQueries({
        queryKey: ["courses", courseId, "signed-urls"],
      })
    },
  })

  const deleteMutation = useMutation({
    mutationFn: (id: string) =>
      api.delete(`/courses/${courseId}/signed-urls/${id}`),
    onSuccess: () => {
      queryClient.invalidateQueries({
        queryKey: ["courses", courseId, "signed-urls"],
      })
    },
  })

  return (
    <Card>
      <CardHeader>
        <CardTitle>Invite Links</CardTitle>
        <CardDescription>
          Generate signed URLs for students to join this course
        </CardDescription>
      </CardHeader>
      <CardContent className="space-y-4">
        <Button
          onClick={() => createMutation.mutate({ expires_in_hours: 168 })}
          disabled={createMutation.isPending}
        >
          {createMutation.isPending ? "Generating..." : "Generate Link (7 days)"}
        </Button>

        {isLoading && (
          <div className="space-y-2">
            <Skeleton className="h-10 w-full" />
            <Skeleton className="h-10 w-full" />
          </div>
        )}

        <div className="space-y-3">
          {links?.map((link) => (
            <div
              key={link.id}
              className="flex items-center justify-between py-2 border-b last:border-0"
            >
              <div className="space-y-1 flex-1 min-w-0">
                <code className="text-xs bg-muted px-2 py-1 rounded block truncate">
                  {window.location.origin}/api/join/{link.token}
                </code>
                <div className="flex gap-3 text-xs text-muted-foreground">
                  <span>Expires: {new Date(link.expires_at).toLocaleDateString()}</span>
                  <span>Used: {link.use_count}{link.max_uses ? `/${link.max_uses}` : ""}</span>
                </div>
              </div>
              <div className="flex gap-2 ml-2">
                <Button
                  variant="outline"
                  size="sm"
                  onClick={() => {
                    navigator.clipboard.writeText(
                      `${window.location.origin}/api/join/${link.token}`,
                    )
                  }}
                >
                  Copy
                </Button>
                <Button
                  variant="ghost"
                  size="sm"
                  onClick={() => deleteMutation.mutate(link.id)}
                >
                  Revoke
                </Button>
              </div>
            </div>
          ))}
        </div>
      </CardContent>
    </Card>
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
    } catch (e) {
      setResults([])
    } finally {
      setSearching(false)
    }
  }

  return (
    <div className="space-y-4">
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

      <ChunkBrowser courseId={courseId} />
    </div>
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

function ConversationsPanel({ courseId }: { courseId: string }) {
  const { data: conversations, isLoading } = useQuery(allConversationsQuery(courseId))
  const [expandedId, setExpandedId] = useState<string | null>(null)
  const queryClient = useQueryClient()

  const pinMutation = useMutation({
    mutationFn: ({ cid, pinned }: { cid: string; pinned: boolean }) =>
      api.put(`/courses/${courseId}/conversations/${cid}/pin`, { pinned }),
    onSuccess: () => {
      queryClient.invalidateQueries({
        queryKey: ["courses", courseId, "conversations"],
      })
    },
  })

  // Group conversations by user
  const grouped = new Map<string, { label: string; conversations: ConversationWithUser[] }>()
  for (const conv of conversations || []) {
    const key = conv.user_id
    if (!grouped.has(key)) {
      grouped.set(key, {
        label: conv.user_display_name || conv.user_eppn || "Unknown",
        conversations: [],
      })
    }
    grouped.get(key)!.conversations.push(conv)
  }

  return (
    <div className="space-y-4">
      <Card>
        <CardHeader>
          <CardTitle>Student Conversations</CardTitle>
          <CardDescription>
            View all student conversations. Pin good answers to make them visible to all students.
          </CardDescription>
        </CardHeader>
        <CardContent>
          {isLoading && (
            <div className="space-y-2">
              <Skeleton className="h-10 w-full" />
              <Skeleton className="h-10 w-full" />
              <Skeleton className="h-10 w-full" />
            </div>
          )}
          {!isLoading && conversations?.length === 0 && (
            <p className="text-muted-foreground text-sm">No conversations yet.</p>
          )}
          <div className="space-y-6">
            {Array.from(grouped.entries()).map(([userId, group]) => (
              <div key={userId}>
                <h4 className="font-medium text-sm mb-2">{group.label}</h4>
                <div className="space-y-1">
                  {group.conversations.map((conv) => (
                    <div key={conv.id}>
                      <div
                        className={`flex items-center justify-between py-2 px-3 rounded cursor-pointer ${
                          expandedId === conv.id ? "bg-secondary" : "hover:bg-muted"
                        }`}
                        onClick={() => setExpandedId(expandedId === conv.id ? null : conv.id)}
                      >
                        <div className="flex items-center gap-2 min-w-0 flex-1">
                          <span className="text-sm truncate">
                            {conv.title || "Untitled conversation"}
                          </span>
                          <span className="text-xs text-muted-foreground shrink-0">
                            {conv.message_count || 0} msgs
                          </span>
                          {conv.pinned && (
                            <Badge variant="secondary" className="shrink-0">Pinned</Badge>
                          )}
                        </div>
                        <div className="flex items-center gap-2 shrink-0 ml-2">
                          <span className="text-xs text-muted-foreground">
                            {new Date(conv.updated_at).toLocaleDateString()}
                          </span>
                          <Button
                            variant={conv.pinned ? "default" : "outline"}
                            size="sm"
                            onClick={(e) => {
                              e.stopPropagation()
                              pinMutation.mutate({ cid: conv.id, pinned: !conv.pinned })
                            }}
                          >
                            {conv.pinned ? "Unpin" : "Pin"}
                          </Button>
                        </div>
                      </div>
                      {expandedId === conv.id && (
                        <ConversationExpanded courseId={courseId} conversationId={conv.id} />
                      )}
                    </div>
                  ))}
                </div>
              </div>
            ))}
          </div>
        </CardContent>
      </Card>
    </div>
  )
}

function ConversationExpanded({ courseId, conversationId }: { courseId: string; conversationId: string }) {
  const { data, isLoading } = useQuery(conversationDetailQuery(courseId, conversationId))
  const queryClient = useQueryClient()
  const [noteContent, setNoteContent] = useState("")
  const [noteForMessage, setNoteForMessage] = useState<string | null>(null)

  const addNoteMutation = useMutation({
    mutationFn: (body: { content: string; message_id?: string }) =>
      api.post<TeacherNote>(`/courses/${courseId}/conversations/${conversationId}/notes`, body),
    onSuccess: () => {
      queryClient.invalidateQueries({
        queryKey: ["courses", courseId, "conversations", conversationId],
      })
      setNoteContent("")
      setNoteForMessage(null)
    },
  })

  const deleteNoteMutation = useMutation({
    mutationFn: (noteId: string) =>
      api.delete(`/courses/${courseId}/conversations/${conversationId}/notes/${noteId}`),
    onSuccess: () => {
      queryClient.invalidateQueries({
        queryKey: ["courses", courseId, "conversations", conversationId],
      })
    },
  })

  if (isLoading) {
    return (
      <div className="ml-4 border-l-2 pl-4 py-2 space-y-2">
        <Skeleton className="h-16 w-full" />
        <Skeleton className="h-16 w-full" />
      </div>
    )
  }

  const messages = data?.messages || []
  const notes = data?.notes || []

  const notesByMessage = new Map<string, TeacherNote[]>()
  const conversationNotes: TeacherNote[] = []
  for (const note of notes) {
    if (note.message_id) {
      const existing = notesByMessage.get(note.message_id) || []
      existing.push(note)
      notesByMessage.set(note.message_id, existing)
    } else {
      conversationNotes.push(note)
    }
  }

  const handleAddNote = (messageId?: string) => {
    if (!noteContent.trim()) return
    addNoteMutation.mutate({
      content: noteContent,
      message_id: messageId || undefined,
    })
  }

  return (
    <div className="ml-4 border-l-2 pl-4 py-2 space-y-3 max-h-[600px] overflow-y-auto">
      {conversationNotes.map((note) => (
        <NoteDisplay key={note.id} note={note} onDelete={() => deleteNoteMutation.mutate(note.id)} />
      ))}

      {messages.map((msg) => (
        <React.Fragment key={msg.id}>
          <div
            className={`rounded px-3 py-2 text-sm ${
              msg.role === "user" ? "bg-primary/10" : "bg-muted"
            }`}
          >
            <span className="text-xs font-medium text-muted-foreground block mb-1">
              {msg.role === "user" ? "Student" : "Assistant"}
            </span>
            {msg.role === "user" ? (
              <p className="whitespace-pre-wrap">{msg.content}</p>
            ) : (
              <div className="prose prose-sm dark:prose-invert max-w-none">
                <Markdown remarkPlugins={[remarkGfm]}>{msg.content}</Markdown>
              </div>
            )}
            <button
              className="text-xs text-muted-foreground hover:text-foreground mt-1 underline"
              onClick={() => setNoteForMessage(noteForMessage === msg.id ? null : msg.id)}
            >
              Add note
            </button>
          </div>

          {notesByMessage.get(msg.id)?.map((note) => (
            <NoteDisplay key={note.id} note={note} onDelete={() => deleteNoteMutation.mutate(note.id)} />
          ))}

          {noteForMessage === msg.id && (
            <div className="flex gap-2">
              <Textarea
                value={noteContent}
                onChange={(e) => setNoteContent(e.target.value)}
                placeholder="Add a teacher's note for this message..."
                rows={2}
                className="flex-1"
              />
              <div className="flex flex-col gap-1">
                <Button
                  size="sm"
                  onClick={() => handleAddNote(msg.id)}
                  disabled={addNoteMutation.isPending || !noteContent.trim()}
                >
                  Save
                </Button>
                <Button
                  size="sm"
                  variant="ghost"
                  onClick={() => { setNoteForMessage(null); setNoteContent("") }}
                >
                  Cancel
                </Button>
              </div>
            </div>
          )}
        </React.Fragment>
      ))}

      <Separator />
      <div className="space-y-2">
        <Label className="text-xs">Add a general note to this conversation</Label>
        <div className="flex gap-2">
          <Textarea
            value={noteForMessage === null ? noteContent : ""}
            onChange={(e) => { setNoteForMessage(null); setNoteContent(e.target.value) }}
            placeholder="Teacher's note visible to all students when pinned..."
            rows={2}
            className="flex-1"
          />
          <Button
            size="sm"
            className="self-end"
            onClick={() => handleAddNote()}
            disabled={addNoteMutation.isPending || !noteContent.trim() || noteForMessage !== null}
          >
            Add Note
          </Button>
        </div>
      </div>
    </div>
  )
}

function NoteDisplay({ note, onDelete }: { note: TeacherNote; onDelete: () => void }) {
  return (
    <div className="bg-amber-50 dark:bg-amber-950/30 border border-amber-200 dark:border-amber-800 rounded px-3 py-2">
      <div className="flex items-center justify-between mb-1">
        <div className="flex items-center gap-2">
          <Badge variant="outline" className="text-xs border-amber-300 dark:border-amber-700 text-amber-700 dark:text-amber-300">
            Teacher note
          </Badge>
          {note.author_display_name && (
            <span className="text-xs text-muted-foreground">{note.author_display_name}</span>
          )}
        </div>
        <Button variant="ghost" size="sm" className="h-6 px-2 text-xs" onClick={onDelete}>
          Delete
        </Button>
      </div>
      <div className="prose prose-sm dark:prose-invert max-w-none">
        <Markdown remarkPlugins={[remarkGfm]}>{note.content}</Markdown>
      </div>
    </div>
  )
}

function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`
}
