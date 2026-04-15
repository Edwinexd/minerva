import { createFileRoute } from "@tanstack/react-router"
import { RelativeTime } from "@/components/relative-time"
import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query"
import { allConversationsQuery, conversationDetailQuery, courseFeedbackStatsQuery, popularTopicsQuery } from "@/lib/queries"
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
import type { ConversationWithUser, MessageFeedback, TeacherNote } from "@/lib/types"
import { FEEDBACK_CATEGORIES } from "@/lib/types"

export const Route = createFileRoute("/teacher/courses/$courseId/conversations")({
  component: ConversationsPage,
})

function categoryLabel(value: string | null): string {
  if (!value) return "Other"
  return FEEDBACK_CATEGORIES.find((c) => c.value === value)?.label ?? value
}

function ConversationsPage() {
  const { courseId } = Route.useParams()
  const { data: conversations, isLoading } = useQuery(allConversationsQuery(courseId))
  const { data: topics, isLoading: topicsLoading } = useQuery(popularTopicsQuery(courseId))
  const { data: feedbackStats } = useQuery(courseFeedbackStatsQuery(courseId))
  const [expandedId, setExpandedId] = useState<string | null>(null)
  const [selectedTopic, setSelectedTopic] = useState<string | null>(null)
  const [activeTab, setActiveTab] = useState<"all" | "flagged">("all")
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

  const displayConversations = useMemo(() => {
    let list = topicConvIds
      ? (conversations || []).filter((c) => topicConvIds.has(c.id))
      : (conversations || [])

    if (activeTab === "flagged") {
      list = list
        .filter((c) => (c.unaddressed_down ?? 0) > 0)
        .sort((a, b) => (b.unaddressed_down ?? 0) - (a.unaddressed_down ?? 0))
    }
    return list
  }, [conversations, topicConvIds, activeTab])

  const flaggedCount = useMemo(
    () => (conversations || []).filter((c) => (c.unaddressed_down ?? 0) > 0).length,
    [conversations],
  )

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
      {feedbackStats && (feedbackStats.total_up > 0 || feedbackStats.total_down > 0) && (
        <Card>
          <CardHeader>
            <CardTitle className="text-base">Student Feedback</CardTitle>
          </CardHeader>
          <CardContent className="space-y-3">
            <div className="flex items-center gap-6 text-sm">
              <div>
                <span className="text-2xl font-semibold">{feedbackStats.total_up}</span>
                <span className="ml-1.5 text-muted-foreground">helpful</span>
              </div>
              <div>
                <span className="text-2xl font-semibold">{feedbackStats.total_down}</span>
                <span className="ml-1.5 text-muted-foreground">flagged</span>
              </div>
            </div>
            {feedbackStats.categories.length > 0 && (
              <div className="flex flex-wrap gap-2">
                {feedbackStats.categories.map((c) => (
                  <Badge key={c.category ?? "null"} variant="secondary" className="text-xs font-normal">
                    {categoryLabel(c.category)} &middot; {c.count}
                  </Badge>
                ))}
              </div>
            )}
          </CardContent>
        </Card>
      )}

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
        <CardContent className="space-y-4">
          <div className="flex gap-2">
            <Button
              variant={activeTab === "all" ? "default" : "outline"}
              size="sm"
              onClick={() => setActiveTab("all")}
            >
              All
            </Button>
            <Button
              variant={activeTab === "flagged" ? "default" : "outline"}
              size="sm"
              onClick={() => setActiveTab("flagged")}
            >
              Needs Review
              {flaggedCount > 0 && (
                <Badge variant="destructive" className="ml-1.5 px-1.5 py-0 text-xs">
                  {flaggedCount}
                </Badge>
              )}
            </Button>
          </div>

          {isLoading && (
            <div className="space-y-2">
              <Skeleton className="h-10 w-full" />
              <Skeleton className="h-10 w-full" />
              <Skeleton className="h-10 w-full" />
            </div>
          )}
          {!isLoading && displayConversations.length === 0 && (
            <p className="text-muted-foreground text-sm">
              {activeTab === "flagged"
                ? "No conversations with flagged responses."
                : activeTopic
                  ? "No conversations match this topic."
                  : "No conversations yet."}
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
                          {(conv.feedback_down ?? 0) > 0 && (
                            <Badge variant="outline" className="shrink-0 border-red-300 text-red-600 dark:border-red-700 dark:text-red-400 text-xs">
                              {conv.feedback_down} flagged
                            </Badge>
                          )}
                          {(conv.feedback_up ?? 0) > 0 && (conv.feedback_down ?? 0) === 0 && (
                            <Badge variant="outline" className="shrink-0 border-green-300 text-green-600 dark:border-green-700 dark:text-green-400 text-xs">
                              {conv.feedback_up} helpful
                            </Badge>
                          )}
                        </div>
                        <div className="flex items-center gap-2 shrink-0 ml-2">
                          <span className="text-xs text-muted-foreground">
                            <RelativeTime date={conv.updated_at} />
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

function FeedbackBadges({ feedback }: { feedback: MessageFeedback[] }) {
  const down = feedback.filter((f) => f.rating === "down")
  const up = feedback.filter((f) => f.rating === "up")
  if (down.length === 0 && up.length === 0) return null
  return (
    <div className="flex flex-wrap gap-1 mt-1.5">
      {up.length > 0 && (
        <Badge variant="outline" className="text-xs border-green-300 text-green-700 dark:border-green-700 dark:text-green-300">
          {up.length} helpful
        </Badge>
      )}
      {down.map((f) => (
        <Badge
          key={f.id}
          variant="outline"
          className="text-xs border-red-300 text-red-700 dark:border-red-700 dark:text-red-300"
          title={f.comment ?? undefined}
        >
          {categoryLabel(f.category)}
          {f.user_display_name ? ` (${f.user_display_name})` : ""}
        </Badge>
      ))}
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

  const openNoteForFeedback = (messageId: string, feedback: MessageFeedback[]) => {
    const down = feedback.filter((f) => f.rating === "down")
    const categories = [...new Set(down.map((f) => categoryLabel(f.category)))].join(", ")
    const prefix = categories ? `Correction (re: ${categories}): ` : "Correction: "
    setNoteForMessage(messageId)
    setNoteContent(prefix)
  }

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
  const feedback = data?.feedback || []

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

  const feedbackByMessage = new Map<string, MessageFeedback[]>()
  for (const f of feedback) {
    const existing = feedbackByMessage.get(f.message_id) || []
    existing.push(f)
    feedbackByMessage.set(f.message_id, existing)
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

      {messages.map((msg) => {
        const msgFeedback = feedbackByMessage.get(msg.id) || []
        const hasDownFeedback = msgFeedback.some((f) => f.rating === "down")
        return (
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
              {msg.role === "assistant" && msgFeedback.length > 0 && (
                <FeedbackBadges feedback={msgFeedback} />
              )}
              <div className="flex items-center gap-3 mt-1.5">
                <button
                  className="text-xs text-muted-foreground hover:text-foreground underline"
                  onClick={() => setNoteForMessage(noteForMessage === msg.id ? null : msg.id)}
                >
                  Add note
                </button>
                {msg.role === "assistant" && hasDownFeedback && noteForMessage !== msg.id && (
                  <button
                    className="text-xs text-red-600 hover:text-red-800 dark:text-red-400 dark:hover:text-red-200 underline"
                    onClick={() => openNoteForFeedback(msg.id, msgFeedback)}
                  >
                    Add correction note
                  </button>
                )}
              </div>
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
        )
      })}
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
