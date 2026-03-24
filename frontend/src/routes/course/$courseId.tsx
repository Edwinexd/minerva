import { createFileRoute } from "@tanstack/react-router"
import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query"
import {
  courseQuery,
  conversationsQuery,
  conversationMessagesQuery,
} from "@/lib/queries"
import { api } from "@/lib/api"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { Skeleton } from "@/components/ui/skeleton"
import React, { useState, useRef, useEffect, useCallback } from "react"
import type { Conversation, Message } from "@/lib/types"

export const Route = createFileRoute("/course/$courseId")({
  component: ChatPage,
})

function ChatPage() {
  const { courseId } = Route.useParams()
  const { data: course } = useQuery(courseQuery(courseId))
  const { data: conversations, isLoading } = useQuery(conversationsQuery(courseId))
  const queryClient = useQueryClient()
  const [activeConversation, setActiveConversation] = useState<string | null>(null)

  const createConversation = useMutation({
    mutationFn: () =>
      api.post<Conversation>(`/courses/${courseId}/conversations`, {}),
    onSuccess: (conv) => {
      queryClient.invalidateQueries({
        queryKey: ["courses", courseId, "conversations"],
      })
      setActiveConversation(conv.id)
    },
  })

  useEffect(() => {
    if (conversations && conversations.length > 0 && !activeConversation) {
      setActiveConversation(conversations[0].id)
    }
  }, [conversations, activeConversation])

  return (
    <div className="flex h-[calc(100vh-120px)] gap-4">
      <div className="w-64 border-r pr-4 flex flex-col">
        <Button
          className="mb-4"
          onClick={() => createConversation.mutate()}
          disabled={createConversation.isPending}
        >
          New Chat
        </Button>
        <div className="space-y-1 overflow-y-auto flex-1">
          {isLoading && Array.from({ length: 3 }).map((_, i) => (
            <Skeleton key={i} className="h-9 w-full mb-1" />
          ))}
          {conversations?.map((conv) => (
            <button
              key={conv.id}
              onClick={() => setActiveConversation(conv.id)}
              className={`w-full text-left px-3 py-2 rounded text-sm truncate ${
                activeConversation === conv.id
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

      <div className="flex-1 flex flex-col">
        {activeConversation ? (
          <ChatWindow courseId={courseId} conversationId={activeConversation} />
        ) : (
          <div className="flex-1 flex items-center justify-center text-muted-foreground">
            {conversations?.length === 0
              ? "Create a new chat to get started."
              : "Select a conversation."}
          </div>
        )}
      </div>
    </div>
  )
}

function ChatWindow({
  courseId,
  conversationId,
}: {
  courseId: string
  conversationId: string
}) {
  const { data: messages, isLoading } = useQuery(
    conversationMessagesQuery(courseId, conversationId),
  )
  const queryClient = useQueryClient()
  const [input, setInput] = useState("")
  const [streaming, setStreaming] = useState(false)
  const [streamedTokens, setStreamedTokens] = useState("")
  const [pendingUserMsg, setPendingUserMsg] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)
  const messagesEndRef = useRef<HTMLDivElement>(null)

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

    const devUser = localStorage.getItem("minerva-dev-user")
    const headers: Record<string, string> = { "Content-Type": "application/json" }
    if (devUser) headers["X-Dev-User"] = devUser

    try {
      const response = await fetch(
        `/api/courses/${courseId}/conversations/${conversationId}/message`,
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
      queryClient.invalidateQueries({
        queryKey: ["courses", courseId, "conversations", conversationId],
      })
      queryClient.invalidateQueries({
        queryKey: ["courses", courseId, "conversations"],
      })
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
      <div className="flex-1 overflow-y-auto pr-4">
        <div className="space-y-4 py-4">
          {isLoading && Array.from({ length: 3 }).map((_, i) => (
            <div key={i} className={`flex ${i % 2 === 0 ? "justify-end" : "justify-start"}`}>
              <Skeleton className="h-12 w-64 rounded-lg" />
            </div>
          ))}
          {messages?.map((msg) => (
            <ChatBubble key={msg.id} message={msg} />
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
                  <p className="text-sm whitespace-pre-wrap">{streamedTokens}</p>
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
          {error && (
            <p className="text-sm text-destructive text-center">{error}</p>
          )}
          <div ref={messagesEndRef} />
        </div>
      </div>

      <form onSubmit={handleSubmit} className="flex gap-2 pt-4 border-t">
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
    </>
  )
}

function ChatBubble({ message }: { message: Message }) {
  const isUser = message.role === "user"

  return (
    <div className={`flex ${isUser ? "justify-end" : "justify-start"}`}>
      <div
        className={`rounded-lg px-4 py-2 max-w-[80%] ${
          isUser ? "bg-primary text-primary-foreground" : "bg-muted"
        }`}
      >
        <p className="text-sm whitespace-pre-wrap">{message.content}</p>
        {!isUser && message.tokens_prompt != null && (
          <p className="text-xs text-muted-foreground mt-1">
            {message.tokens_prompt + (message.tokens_completion || 0)} tokens
          </p>
        )}
      </div>
    </div>
  )
}
