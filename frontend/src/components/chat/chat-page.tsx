import { Link, useNavigate } from "@tanstack/react-router"
import { useQuery, useQueryClient } from "@tanstack/react-query"
import { useTranslation } from "react-i18next"
import {
  courseQuery,
  conversationsQuery,
  conversationDetailQuery,
  pinnedConversationsQuery,
  userQuery,
} from "@/lib/queries"
import { api } from "@/lib/api"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { Skeleton } from "@/components/ui/skeleton"
import { Badge } from "@/components/ui/badge"
import { ChevronDown, ChevronUp, Menu, X } from "lucide-react"
import Markdown from "react-markdown"
import remarkGfm from "remark-gfm"
import React, { useState, useRef, useEffect, useCallback } from "react"
import type { Message, MessageFeedback, TeacherNote } from "@/lib/types"
import { FeedbackControls } from "@/components/message-feedback"
import { PrivacyAckBanner } from "@/components/privacy-ack"
import { useDocumentTitle } from "@/lib/use-document-title"

export function ChatRouteComponent({
  useParams,
}: {
  useParams: () => { courseId: string; conversationId: string }
}) {
  const { courseId, conversationId } = useParams()
  return <ChatPage courseId={courseId} conversationId={conversationId} />
}

export function NewChatRouteComponent({
  useParams,
}: {
  useParams: () => { courseId: string }
}) {
  const { courseId } = useParams()
  return <ChatPage courseId={courseId} conversationId={null} />
}

function ChatPage({
  courseId,
  conversationId,
}: {
  courseId: string
  conversationId: string | null
}) {
  const navigate = useNavigate()
  const { t } = useTranslation("student")
  const { t: tCommon } = useTranslation("common")
  const { data: course } = useQuery(courseQuery(courseId))
  const { data: conversations, isLoading: convLoading } = useQuery(conversationsQuery(courseId))
  const { data: pinned, isLoading: pinnedLoading } = useQuery(pinnedConversationsQuery(courseId))

  useDocumentTitle(course ? tCommon("pageTitles.course", { course: course.name }) : null)

  const isPinnedView = conversationId !== null &&
    pinned?.some((p) => p.id === conversationId) &&
    !conversations?.some((c) => c.id === conversationId)

  const [sidebarOpen, setSidebarOpen] = useState(false)
  const [prevConversationId, setPrevConversationId] = useState(conversationId)

  // Close the sidebar whenever the active conversation changes.
  // Done during render (not in an effect) so React can batch it with the
  // parent render instead of triggering an extra cascade.
  if (prevConversationId !== conversationId) {
    setPrevConversationId(conversationId)
    setSidebarOpen(false)
  }

  return (
    <div className="relative flex h-[calc(100vh-120px)] gap-4">
      <Button
        variant="outline"
        size="sm"
        className="md:hidden absolute top-0 left-0 z-20"
        onClick={() => setSidebarOpen(true)}
        aria-label={t("sidebar.openConversations")}
      >
        <Menu className="w-4 h-4" />
      </Button>
      {sidebarOpen && (
        <div
          className="md:hidden fixed inset-0 z-30 bg-background/60"
          onClick={() => setSidebarOpen(false)}
        />
      )}
      <div
        className={`${
          sidebarOpen
            ? "fixed inset-y-0 left-0 z-40 w-72 bg-background border-r p-4 flex flex-col md:static md:inset-auto md:w-64 md:p-0 md:pr-4 md:bg-transparent"
            : "hidden md:flex md:w-64 border-r pr-4 flex-col"
        }`}
      >
        <div className="md:hidden flex justify-end mb-2">
          <Button
            variant="ghost"
            size="sm"
            onClick={() => setSidebarOpen(false)}
            aria-label={t("sidebar.closeConversations")}
          >
            <X className="w-4 h-4" />
          </Button>
        </div>
        <Button
          className="mb-4"
          onClick={() => navigate({ to: "/course/$courseId/new", params: { courseId } })}
          disabled={conversationId === null}
        >
          {t("sidebar.newChat")}
        </Button>
        <div className="space-y-1 overflow-y-auto flex-1">
          {convLoading && Array.from({ length: 3 }).map((_, i) => (
            <Skeleton key={i} className="h-9 w-full mb-1" />
          ))}
          {conversations?.map((conv) => (
            <Link
              key={conv.id}
              to="/course/$courseId/$conversationId"
              params={{ courseId, conversationId: conv.id }}
              className={`block w-full text-left px-3 py-2 rounded text-sm truncate ${
                conversationId === conv.id
                  ? "bg-secondary text-secondary-foreground"
                  : "hover:bg-muted"
              }`}
            >
              {conv.pinned && <span className="mr-1" title={t("sidebar.pinned")}>*</span>}
              {conv.title || t("sidebar.newConversation")}
            </Link>
          ))}

          {pinned && pinned.length > 0 && (
            <>
              <div className="text-xs font-medium text-muted-foreground pt-3 pb-1 border-t mt-2">
                {t("sidebar.pinnedByTeacher")}
              </div>
              {pinnedLoading && <Skeleton className="h-9 w-full mb-1" />}
              {pinned
                .filter((p) => !conversations?.some((c) => c.id === p.id))
                .map((conv) => (
                  <Link
                    key={conv.id}
                    to="/course/$courseId/$conversationId"
                    params={{ courseId, conversationId: conv.id }}
                    className={`block w-full text-left px-3 py-2 rounded text-sm truncate ${
                      conversationId === conv.id
                        ? "bg-secondary text-secondary-foreground"
                        : "hover:bg-muted"
                    }`}
                  >
                    <span className="text-muted-foreground text-xs">
                      {conv.user_display_name || conv.user_eppn || t("sidebar.studentFallback")}
                    </span>
                    <span className="block">{conv.title || t("sidebar.conversation")}</span>
                  </Link>
                ))}
            </>
          )}
        </div>
        {course && (
          <div className="text-xs text-muted-foreground pt-2 border-t mt-2">
            {course.name}
          </div>
        )}
      </div>

      <div className="flex-1 flex flex-col min-w-0 pl-10 md:pl-0">
        <ChatWindow
          courseId={courseId}
          conversationId={conversationId}
          readOnly={isPinnedView}
        />
      </div>
    </div>
  )
}

function ChatWindow({
  courseId,
  conversationId,
  readOnly = false,
}: {
  courseId: string
  conversationId: string | null
  readOnly?: boolean
}) {
  const navigate = useNavigate()
  const { t } = useTranslation("student")
  const { data, isLoading } = useQuery({
    ...conversationDetailQuery(courseId, conversationId ?? ""),
    enabled: conversationId !== null,
  })
  const messages = data?.messages
  const notes = data?.notes || []
  const feedback = data?.feedback || []
  const { data: user } = useQuery(userQuery)
  const needsPrivacyAck = !!user && !user.privacy_acknowledged_at
  const queryClient = useQueryClient()

  // Build a map of message_id -> the current user's feedback row (if any)
  // so each ChatBubble knows whether to render thumbs as selected.
  const myFeedbackByMessage = new Map<string, MessageFeedback>()
  if (user) {
    for (const f of feedback) {
      if (f.user_id === user.id) myFeedbackByMessage.set(f.message_id, f)
    }
  }
  const [input, setInput] = useState("")
  const [streaming, setStreaming] = useState(false)
  const [streamedTokens, setStreamedTokens] = useState("")
  const [pendingUserMsg, setPendingUserMsg] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)
  const messagesEndRef = useRef<HTMLDivElement>(null)

  // Index notes by message_id for inline display
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

  const scrollToBottom = useCallback(() => {
    messagesEndRef.current?.scrollIntoView({ behavior: "smooth" })
  }, [])

  useEffect(() => {
    scrollToBottom()
  }, [messages, streamedTokens, scrollToBottom])

  // Reset state when conversation changes
  useEffect(() => {
    setStreaming(false)
    setStreamedTokens("")
    setPendingUserMsg(null)
    setError(null)
    setInput("")
  }, [conversationId])

  const sendMessage = async (content: string, targetConvId: string): Promise<boolean> => {
    setError(null)
    setStreaming(true)
    setStreamedTokens("")
    setPendingUserMsg(content)

    const devUser = localStorage.getItem("minerva-dev-user")
    const headers: Record<string, string> = { "Content-Type": "application/json" }
    if (devUser) headers["X-Dev-User"] = devUser

    let success = true
    try {
      const response = await fetch(
        `/api/courses/${courseId}/conversations/${targetConvId}/message`,
        { method: "POST", headers, body: JSON.stringify({ content }) },
      )

      if (!response.ok) {
        const body = await response.json().catch(() => ({ error: response.statusText }))
        throw new Error(body.error || response.statusText)
      }

      const reader = response.body?.getReader()
      const decoder = new TextDecoder()

      if (reader) {
        let buffer = ""
        while (true) {
          const { done, value } = await reader.read()
          if (done) break
          buffer += decoder.decode(value, { stream: true })

          const lines = buffer.split("\n")
          buffer = lines.pop() || ""

          for (const line of lines) {
            if (line.startsWith("data: ")) {
              try {
                const data = JSON.parse(line.slice(6))
                if (data.type === "token") {
                  setStreamedTokens((prev) => prev + data.token)
                } else if (data.type === "error") {
                  setError(data.error)
                  success = false
                }
              } catch {
                // skip malformed json
              }
            }
          }
        }
      }
    } catch (e) {
      setError(e instanceof Error ? e.message : t("chat.unknownError"))
      success = false
    } finally {
      setStreaming(false)
      setStreamedTokens("")
      setPendingUserMsg(null)
      queryClient.invalidateQueries({
        queryKey: ["courses", courseId, "conversations", targetConvId],
      })
      queryClient.invalidateQueries({
        queryKey: ["courses", courseId, "conversations"],
      })
    }
    return success
  }

  const handleSubmit = (e: React.FormEvent) => {
    e.preventDefault()
    if (!input.trim() || streaming) return
    const msg = input
    setInput("")

    if (conversationId === null) {
      // Lazy-create: client picks the conv id and posts the first message
      // directly. The backend creates the conversation row only after its
      // gate checks (privacy ack, per-course cap, owner cap) pass, so a
      // rejected first message never leaves an "Untitled, 0 msgs" behind.
      const newId = crypto.randomUUID()
      ;(async () => {
        const ok = await sendMessage(msg, newId)
        if (ok) {
          navigate({
            to: "/course/$courseId/$conversationId",
            params: { courseId, conversationId: newId },
            replace: true,
          })
        }
      })()
    } else {
      sendMessage(msg, conversationId)
    }
  }

  return (
    <>
      <div className="flex-1 overflow-y-auto pr-4">
        <div className="space-y-4 py-4">
          {conversationNotes.length > 0 && (
            <div className="space-y-2">
              {conversationNotes.map((note) => (
                <TeacherNoteInline key={note.id} note={note} />
              ))}
            </div>
          )}
          {isLoading && Array.from({ length: 3 }).map((_, i) => (
            <div key={i} className={`flex ${i % 2 === 0 ? "justify-end" : "justify-start"}`}>
              <Skeleton className="h-12 w-64 rounded-lg" />
            </div>
          ))}
          {messages?.map((msg) => (
            <React.Fragment key={msg.id}>
              <ChatBubble
                message={msg}
                courseId={courseId}
                conversationId={conversationId!}
                feedback={myFeedbackByMessage.get(msg.id) ?? null}
                canRate={!readOnly && msg.role === "assistant"}
              />
              {notesByMessage.get(msg.id)?.map((note) => (
                <TeacherNoteInline key={note.id} note={note} />
              ))}
            </React.Fragment>
          ))}
          {pendingUserMsg && (
            <div className="flex justify-end">
              <div className="rounded-lg px-4 py-2 max-w-[80%] bg-primary text-primary-foreground">
                <p className="text-sm whitespace-pre-wrap">{pendingUserMsg}</p>
              </div>
            </div>
          )}
          {streaming && (
            <div className="flex justify-start">
              <div
                className="bg-muted rounded-lg px-4 py-2 max-w-[80%]"
                aria-live="polite"
                aria-atomic="false"
                aria-label={t("chat.assistantResponseLabel")}
              >
                {streamedTokens ? (
                  <MarkdownContent content={streamedTokens} />
                ) : (
                  <div className="flex gap-1" aria-hidden="true">
                    <div className="w-2 h-2 bg-muted-foreground/40 rounded-full animate-bounce [animation-delay:0ms]" />
                    <div className="w-2 h-2 bg-muted-foreground/40 rounded-full animate-bounce [animation-delay:150ms]" />
                    <div className="w-2 h-2 bg-muted-foreground/40 rounded-full animate-bounce [animation-delay:300ms]" />
                  </div>
                )}
              </div>
            </div>
          )}
          {error && (
            <p role="alert" className="text-sm text-destructive text-center">{error}</p>
          )}
          <div ref={messagesEndRef} />
        </div>
      </div>

      {!readOnly && (
        <div className="pt-4 border-t space-y-2">
          {needsPrivacyAck && (
            <PrivacyAckBanner
              onAcknowledge={async () => {
                await api.post("/auth/acknowledge-privacy", {})
                await queryClient.invalidateQueries({ queryKey: ["auth", "me"] })
              }}
            />
          )}
          <form onSubmit={handleSubmit} className="flex gap-2">
            <Input
              value={input}
              onChange={(e) => setInput(e.target.value)}
              placeholder={t("chat.inputPlaceholder")}
              disabled={streaming || needsPrivacyAck}
              className="flex-1"
            />
            <Button
              type="submit"
              disabled={streaming || !input.trim() || needsPrivacyAck}
            >
              {t("chat.send")}
            </Button>
          </form>
          <p className="text-xs text-muted-foreground text-center">
            {t("chat.disclaimerBefore")}
            <Link to="/data-handling" className="underline hover:text-foreground">{t("chat.dataHandlingLink")}</Link>
            {t("chat.disclaimerAfter")}
          </p>
        </div>
      )}
    </>
  )
}

function TeacherNoteInline({ note }: { note: TeacherNote }) {
  const { t } = useTranslation("student")
  return (
    <div className="flex justify-center">
      <div className="bg-amber-50 dark:bg-amber-950/30 border border-amber-200 dark:border-amber-800 rounded-lg px-4 py-2 max-w-[80%]">
        <div className="flex items-center gap-2 mb-1">
          <Badge variant="outline" className="text-xs border-amber-300 dark:border-amber-700 text-amber-700 dark:text-amber-300">
            {t("message.teacherNote")}
          </Badge>
          {note.author_display_name && (
            <span className="text-xs text-muted-foreground">{note.author_display_name}</span>
          )}
        </div>
        <div className="prose prose-sm dark:prose-invert max-w-none">
          <Markdown remarkPlugins={[remarkGfm]}>{note.content}</Markdown>
        </div>
      </div>
    </div>
  )
}

function MarkdownContent({ content, className }: { content: string; className?: string }) {
  return (
    <div className={`prose prose-sm dark:prose-invert max-w-none ${className || ""}`}>
      <Markdown remarkPlugins={[remarkGfm]}>{content}</Markdown>
    </div>
  )
}

function ChatBubble({
  message,
  courseId,
  conversationId,
  feedback,
  canRate,
}: {
  message: Message
  courseId: string
  conversationId: string
  feedback: MessageFeedback | null
  canRate: boolean
}) {
  const { t } = useTranslation("student")
  const isUser = message.role === "user"
  const [showSources, setShowSources] = useState(false)
  const chunks: string[] | null = message.chunks_used as string[] | null

  return (
    <div className={`flex ${isUser ? "justify-end" : "justify-start"}`}>
      <div
        className={`rounded-lg px-4 py-2 max-w-[80%] ${
          isUser ? "bg-primary text-primary-foreground" : "bg-muted"
        }`}
      >
        {isUser ? (
          <p className="text-sm whitespace-pre-wrap">{message.content}</p>
        ) : (
          <MarkdownContent content={message.content} />
        )}
        {!isUser && (
          <div className="flex items-center gap-1 mt-2 text-xs text-muted-foreground flex-wrap">
            {message.tokens_prompt != null && (
              <span>
                {t("message.tokensUsed", {
                  count: message.tokens_prompt + (message.tokens_completion || 0),
                })}
                {message.generation_ms != null &&
                  t("message.generationTime", {
                    seconds: (message.generation_ms / 1000).toFixed(1),
                  })}
                {chunks && chunks.length > 0 && t("message.usingSuffix")}
              </span>
            )}
            {chunks && chunks.length > 0 && (
              <>
                <button
                  className="underline hover:text-foreground"
                  onClick={() => setShowSources(!showSources)}
                >
                  {t("message.source", { count: chunks.length })}
                  {showSources ? <ChevronUp className="inline w-3 h-3 ml-0.5" /> : <ChevronDown className="inline w-3 h-3 ml-0.5" />}
                </button>
                {message.retrieval_count != null && message.retrieval_count > 1 && (
                  <span>{t("message.acrossRetrievals", { count: message.retrieval_count })}</span>
                )}
              </>
            )}
            {canRate && (
              <FeedbackControls
                courseId={courseId}
                conversationId={conversationId}
                messageId={message.id}
                current={feedback}
              />
            )}
          </div>
        )}
        {showSources && chunks && (
          <div className="mt-2 space-y-2 border-t pt-2">
            {chunks.map((chunk, i) => {
              const sourceMatch = chunk.match(/^\[Source: (.+?)\](\n|$)/)
              const source = sourceMatch ? sourceMatch[1] : t("message.unknownSource")
              const hasText = sourceMatch ? chunk.length > sourceMatch[0].length : true
              const text = hasText
                ? (sourceMatch ? chunk.slice(sourceMatch[0].length) : chunk)
                : null
              return (
                <div key={i} className="text-xs">
                  <span className="font-medium text-muted-foreground">{source}</span>
                  {text ? (
                    <p className="text-muted-foreground/80 mt-0.5 line-clamp-3">{text}</p>
                  ) : (
                    <p className="text-muted-foreground/60 mt-0.5 italic">{t("message.sourceUnavailable")}</p>
                  )}
                </div>
              )
            })}
          </div>
        )}
      </div>
    </div>
  )
}
