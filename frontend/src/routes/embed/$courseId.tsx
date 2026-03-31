import { createFileRoute } from "@tanstack/react-router"
import React, { useState, useRef, useEffect, useCallback } from "react"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { Skeleton } from "@/components/ui/skeleton"
import { ChevronDown, ChevronUp } from "lucide-react"
import Markdown from "react-markdown"
import remarkGfm from "remark-gfm"

// -- Types for embed API responses --

interface EmbedCourse {
  id: string
  name: string
  description: string | null
}

interface EmbedConversation {
  id: string
  course_id: string
  title: string | null
  created_at: string
  updated_at: string
}

interface EmbedMessage {
  id: string
  role: "user" | "assistant"
  content: string
  chunks_used: string[] | null
  model_used: string | null
  created_at: string
}

interface EmbedConversationDetail {
  messages: EmbedMessage[]
}

// -- Route definition --

export const Route = createFileRoute("/embed/$courseId")({
  component: EmbedPage,
})

/** Read `?token=...` from the URL. */
function useToken(): string | null {
  const [token] = useState(() => {
    const params = new URLSearchParams(window.location.search)
    return params.get("token")
  })
  return token
}

/** Thin wrapper around fetch for the embed API. */
async function embedGet<T>(path: string, token: string): Promise<T> {
  const sep = path.includes("?") ? "&" : "?"
  const res = await fetch(`/api/embed${path}${sep}token=${encodeURIComponent(token)}`)
  if (!res.ok) {
    const body = await res.json().catch(() => ({ error: res.statusText }))
    throw new Error(body.error || res.statusText)
  }
  return res.json()
}

async function embedPost<T>(path: string, token: string, body?: unknown): Promise<T> {
  const sep = path.includes("?") ? "&" : "?"
  const res = await fetch(`/api/embed${path}${sep}token=${encodeURIComponent(token)}`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(body ?? {}),
  })
  if (!res.ok) {
    const b = await res.json().catch(() => ({ error: res.statusText }))
    throw new Error(b.error || res.statusText)
  }
  return res.json()
}

// -- Main page --

function EmbedPage() {
  const { courseId } = Route.useParams()
  const token = useToken()

  const [course, setCourse] = useState<EmbedCourse | null>(null)
  const [conversations, setConversations] = useState<EmbedConversation[]>([])
  const [activeConvId, setActiveConvId] = useState<string | null>(null)
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)

  // Load course and conversations on mount.
  useEffect(() => {
    if (!token) {
      setError("Missing authentication token.")
      setLoading(false)
      return
    }
    let cancelled = false
    ;(async () => {
      try {
        const [c, convs] = await Promise.all([
          embedGet<EmbedCourse>(`/course/${courseId}`, token),
          embedGet<EmbedConversation[]>(`/course/${courseId}/conversations`, token),
        ])
        if (cancelled) return
        setCourse(c)
        setConversations(convs)
        if (convs.length > 0) {
          setActiveConvId(convs[0].id)
        }
      } catch (e) {
        if (!cancelled) setError(e instanceof Error ? e.message : "Failed to load")
      } finally {
        if (!cancelled) setLoading(false)
      }
    })()
    return () => { cancelled = true }
  }, [courseId, token])

  const createConversation = async () => {
    if (!token) return
    try {
      const conv = await embedPost<EmbedConversation>(`/course/${courseId}/conversations`, token)
      setConversations((prev) => [conv, ...prev])
      setActiveConvId(conv.id)
    } catch (e) {
      setError(e instanceof Error ? e.message : "Failed to create conversation")
    }
  }

  const refreshConversations = async () => {
    if (!token) return
    try {
      const convs = await embedGet<EmbedConversation[]>(`/course/${courseId}/conversations`, token)
      setConversations(convs)
    } catch {
      // Silent refresh failure
    }
  }

  if (!token) {
    return (
      <div className="flex items-center justify-center h-dvh bg-background text-foreground">
        <p className="text-destructive">Missing authentication token.</p>
      </div>
    )
  }

  if (loading) {
    return (
      <div className="flex items-center justify-center h-dvh bg-background text-foreground">
        <div className="flex gap-1">
          <div className="w-2 h-2 bg-muted-foreground/40 rounded-full animate-bounce [animation-delay:0ms]" />
          <div className="w-2 h-2 bg-muted-foreground/40 rounded-full animate-bounce [animation-delay:150ms]" />
          <div className="w-2 h-2 bg-muted-foreground/40 rounded-full animate-bounce [animation-delay:300ms]" />
        </div>
      </div>
    )
  }

  if (error && !course) {
    return (
      <div className="flex items-center justify-center h-dvh bg-background text-foreground">
        <p className="text-destructive">{error}</p>
      </div>
    )
  }

  return (
    <div className="flex h-dvh bg-background text-foreground">
      {/* Sidebar */}
      <div className="w-56 border-r flex flex-col p-3">
        <Button size="sm" className="mb-3" onClick={createConversation}>
          New Chat
        </Button>
        <div className="space-y-1 overflow-y-auto flex-1">
          {conversations.map((conv) => (
            <button
              key={conv.id}
              onClick={() => setActiveConvId(conv.id)}
              className={`block w-full text-left px-3 py-2 rounded text-sm truncate ${
                activeConvId === conv.id
                  ? "bg-secondary text-secondary-foreground"
                  : "hover:bg-muted"
              }`}
            >
              {conv.title || "New conversation"}
            </button>
          ))}
        </div>
        {course && (
          <div className="text-xs text-muted-foreground pt-2 border-t mt-2">
            {course.name}
          </div>
        )}
      </div>

      {/* Chat area */}
      <div className="flex-1 flex flex-col">
        {activeConvId ? (
          <EmbedChatWindow
            courseId={courseId}
            conversationId={activeConvId}
            token={token}
            onMessageSent={refreshConversations}
          />
        ) : (
          <div className="flex-1 flex items-center justify-center text-muted-foreground">
            <p>Create a new conversation to get started.</p>
          </div>
        )}
      </div>
    </div>
  )
}

// -- Chat window --

function EmbedChatWindow({
  courseId,
  conversationId,
  token,
  onMessageSent,
}: {
  courseId: string
  conversationId: string
  token: string
  onMessageSent: () => void
}) {
  const [messages, setMessages] = useState<EmbedMessage[]>([])
  const [loading, setLoading] = useState(true)
  const [input, setInput] = useState("")
  const [streaming, setStreaming] = useState(false)
  const [streamedTokens, setStreamedTokens] = useState("")
  const [pendingUserMsg, setPendingUserMsg] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)
  const messagesEndRef = useRef<HTMLDivElement>(null)

  // Load messages when conversation changes.
  useEffect(() => {
    let cancelled = false
    setLoading(true)
    setStreaming(false)
    setStreamedTokens("")
    setPendingUserMsg(null)
    setError(null)
    setInput("")

    embedGet<EmbedConversationDetail>(`/course/${courseId}/conversations/${conversationId}`, token)
      .then((data) => {
        if (!cancelled) {
          setMessages(data.messages)
          setLoading(false)
        }
      })
      .catch((e) => {
        if (!cancelled) {
          setError(e instanceof Error ? e.message : "Failed to load messages")
          setLoading(false)
        }
      })

    return () => { cancelled = true }
  }, [courseId, conversationId, token])

  const scrollToBottom = useCallback(() => {
    messagesEndRef.current?.scrollIntoView({ behavior: "smooth" })
  }, [])

  useEffect(() => {
    scrollToBottom()
  }, [messages, streamedTokens, scrollToBottom])

  const sendMessage = async (content: string) => {
    setError(null)
    setStreaming(true)
    setStreamedTokens("")
    setPendingUserMsg(content)

    try {
      const response = await fetch(
        `/api/embed/course/${courseId}/conversations/${conversationId}/message`,
        {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify({ content, token }),
        },
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
                }
              } catch {
                // skip malformed json
              }
            }
          }
        }
      }
    } catch (e) {
      setError(e instanceof Error ? e.message : "Unknown error")
    } finally {
      setStreaming(false)
      setStreamedTokens("")
      setPendingUserMsg(null)
      // Reload messages
      try {
        const data = await embedGet<EmbedConversationDetail>(
          `/course/${courseId}/conversations/${conversationId}`,
          token,
        )
        setMessages(data.messages)
      } catch {
        // Silent
      }
      onMessageSent()
    }
  }

  const handleSubmit = (e: React.FormEvent) => {
    e.preventDefault()
    if (!input.trim() || streaming) return
    const msg = input
    setInput("")
    sendMessage(msg)
  }

  return (
    <>
      <div className="flex-1 overflow-y-auto px-4">
        <div className="space-y-4 py-4">
          {loading &&
            Array.from({ length: 3 }).map((_, i) => (
              <div key={i} className={`flex ${i % 2 === 0 ? "justify-end" : "justify-start"}`}>
                <Skeleton className="h-12 w-64 rounded-lg" />
              </div>
            ))}
          {messages.map((msg) => (
            <EmbedChatBubble key={msg.id} message={msg} />
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
              <div className="bg-muted rounded-lg px-4 py-2 max-w-[80%]">
                {streamedTokens ? (
                  <MarkdownContent content={streamedTokens} />
                ) : (
                  <div className="flex gap-1">
                    <div className="w-2 h-2 bg-muted-foreground/40 rounded-full animate-bounce [animation-delay:0ms]" />
                    <div className="w-2 h-2 bg-muted-foreground/40 rounded-full animate-bounce [animation-delay:150ms]" />
                    <div className="w-2 h-2 bg-muted-foreground/40 rounded-full animate-bounce [animation-delay:300ms]" />
                  </div>
                )}
              </div>
            </div>
          )}
          {error && <p className="text-sm text-destructive text-center">{error}</p>}
          <div ref={messagesEndRef} />
        </div>
      </div>

      <div className="p-4 border-t space-y-2">
        <form onSubmit={handleSubmit} className="flex gap-2">
          <Input
            value={input}
            onChange={(e) => setInput(e.target.value)}
            placeholder="Ask about the course materials..."
            disabled={streaming}
            className="flex-1"
          />
          <Button type="submit" disabled={streaming || !input.trim()}>
            Send
          </Button>
        </form>
        <p className="text-xs text-muted-foreground text-center">
          Responses are generated by AI and may be inaccurate. Verify important information with
          course materials or your instructor.
        </p>
      </div>
    </>
  )
}

// -- Shared components --

function MarkdownContent({ content }: { content: string }) {
  return (
    <div className="prose prose-sm dark:prose-invert max-w-none">
      <Markdown remarkPlugins={[remarkGfm]}>{content}</Markdown>
    </div>
  )
}

function EmbedChatBubble({ message }: { message: EmbedMessage }) {
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
        {!isUser && chunks && chunks.length > 0 && (
          <div className="mt-2 text-xs text-muted-foreground">
            <button
              className="underline hover:text-foreground"
              onClick={() => setShowSources(!showSources)}
            >
              {chunks.length} source{chunks.length > 1 ? "s" : ""}
              {showSources ? (
                <ChevronUp className="inline w-3 h-3 ml-0.5" />
              ) : (
                <ChevronDown className="inline w-3 h-3 ml-0.5" />
              )}
            </button>
          </div>
        )}
        {showSources && chunks && (
          <div className="mt-2 space-y-2 border-t pt-2">
            {chunks.map((chunk, i) => {
              const sourceMatch = chunk.match(/^\[Source: (.+?)\](\n|$)/)
              const source = sourceMatch ? sourceMatch[1] : "Unknown"
              const hasText = sourceMatch ? chunk.length > sourceMatch[0].length : true
              const text = hasText
                ? sourceMatch
                  ? chunk.slice(sourceMatch[0].length)
                  : chunk
                : null
              return (
                <div key={i} className="text-xs">
                  <span className="font-medium text-muted-foreground">{source}</span>
                  {text ? (
                    <p className="text-muted-foreground/80 mt-0.5 line-clamp-3">{text}</p>
                  ) : (
                    <p className="text-muted-foreground/60 mt-0.5 italic">
                      Source content not available for viewing
                    </p>
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
