import { createFileRoute } from "@tanstack/react-router"
import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query"
import { allConversationsQuery, conversationDetailQuery, popularTopicsQuery } from "@/lib/queries"
import { api } from "@/lib/api"
import { Button } from "@/components/ui/button"
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card"
import { Label } from "@/components/ui/label"
import { Textarea } from "@/components/ui/textarea"
import { Badge } from "@/components/ui/badge"
import { Separator } from "@/components/ui/separator"
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
import React, { useMemo, useState } from "react"
import type { ConversationWithUser, TeacherNote } from "@/lib/types"

export const Route = createFileRoute("/teacher/courses/$courseId/conversations")({
  component: ConversationsPage,
})

function ConversationsPage() {
  const { courseId } = Route.useParams()
  const { data: conversations, isLoading } = useQuery(allConversationsQuery(courseId))
  const { data: topics, isLoading: topicsLoading } = useQuery(popularTopicsQuery(courseId))
  const [expandedId, setExpandedId] = useState<string | null>(null)
  const [selectedTopic, setSelectedTopic] = useState<string | null>(null)
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

  const activeTopic = useMemo(
    () => topics?.find((t) => t.topic === selectedTopic) ?? null,
    [topics, selectedTopic],
  )

  const topicConvIds = useMemo(
    () => activeTopic ? new Set(activeTopic.conversation_ids) : null,
    [activeTopic],
  )
  const displayConversations = topicConvIds
    ? (conversations || []).filter((c) => topicConvIds.has(c.id))
    : (conversations || [])

  const grouped = new Map<string, { label: string; conversations: ConversationWithUser[] }>()
  for (const conv of displayConversations) {
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
      {!topicsLoading && topics && topics.length > 0 && (
        <Card>
          <CardHeader>
            <CardTitle className="text-base">Popular Topics</CardTitle>
            <CardDescription>
              Common themes extracted from student messages across all conversations
            </CardDescription>
          </CardHeader>
          <CardContent className="space-y-4">
            <div className="flex flex-wrap items-center gap-3">
              <Select
                value={selectedTopic ?? ""}
                onValueChange={(v) => setSelectedTopic(v || null)}
              >
                <SelectTrigger className="w-full sm:w-72">
                  <SelectValue placeholder="Filter by topic..." />
                </SelectTrigger>
                <SelectContent>
                  {topics.map((t) => (
                    <SelectItem key={t.topic} value={t.topic}>
                      {t.topic} ({t.conversation_count} convos, {t.unique_users} students)
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
              {selectedTopic && (
                <Button
                  variant="ghost"
                  size="sm"
                  onClick={() => setSelectedTopic(null)}
                >
                  Clear filter
                </Button>
              )}
            </div>
            {activeTopic && (
              <div className="text-sm text-muted-foreground">
                {activeTopic.conversation_count} conversations, {activeTopic.unique_users} students, {activeTopic.total_messages} total messages
              </div>
            )}
          </CardContent>
        </Card>
      )}
      {topicsLoading && (
        <Card>
          <CardHeader>
            <Skeleton className="h-5 w-40" />
            <Skeleton className="h-4 w-64 mt-1" />
          </CardHeader>
          <CardContent>
            <Skeleton className="h-10 w-full sm:w-72" />
          </CardContent>
        </Card>
      )}
      <Card>
        <CardHeader>
          <CardTitle>
            Student Conversations
            {activeTopic && (
              <Badge variant="secondary" className="ml-2 font-normal">
                Filtered: {activeTopic.topic}
              </Badge>
            )}
          </CardTitle>
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
          {!isLoading && displayConversations.length === 0 && (
            <p className="text-muted-foreground text-sm">
              {activeTopic ? "No conversations match this topic." : "No conversations yet."}
            </p>
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
      <Separator />

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
