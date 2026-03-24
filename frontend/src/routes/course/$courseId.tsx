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
import { ScrollArea } from "@/components/ui/scroll-area"
import React, { useState, useRef, useEffect } from "react"
import type { Conversation, Message } from "@/lib/types"

export const Route = createFileRoute("/course/$courseId")({
  component: ChatPage,
})

function ChatPage() {
  const { courseId } = Route.useParams()
  const { data: course } = useQuery(courseQuery(courseId))
  const { data: conversations } = useQuery(conversationsQuery(courseId))
  const queryClient = useQueryClient()
  const [activeConversation, setActiveConversation] = useState<string | null>(
    null,
  )

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

  // Auto-select first conversation
  useEffect(() => {
    if (conversations && conversations.length > 0 && !activeConversation) {
      setActiveConversation(conversations[0].id)
    }
  }, [conversations, activeConversation])

  return (
    <div className="flex h-[calc(100vh-120px)] gap-4">
      {/* Sidebar */}
      <div className="w-64 border-r pr-4 flex flex-col">
        <Button
          className="mb-4"
          onClick={() => createConversation.mutate()}
          disabled={createConversation.isPending}
        >
          New Chat
        </Button>
        <div className="space-y-1 overflow-y-auto flex-1">
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

      {/* Chat area */}
      <div className="flex-1 flex flex-col">
        {activeConversation ? (
          <ChatWindow courseId={courseId} conversationId={activeConversation} />
        ) : (
          <div className="flex-1 flex items-center justify-center text-muted-foreground">
            Select or create a conversation to start chatting.
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
  const [streamingMessage, setStreamingMessage] = useState<string | null>(null)
  const messagesEndRef = useRef<HTMLDivElement>(null)

  const scrollToBottom = () => {
    messagesEndRef.current?.scrollIntoView({ behavior: "smooth" })
  }

  useEffect(() => {
    scrollToBottom()
  }, [messages, streamingMessage])

  const sendMutation = useMutation({
    mutationFn: async (content: string) => {
      setStreamingMessage("")

      const devUser = localStorage.getItem("minerva-dev-user")
      const headers: Record<string, string> = { "Content-Type": "application/json" }
      if (devUser) headers["X-Dev-User"] = devUser

      const response = await fetch(
        `/api/courses/${courseId}/conversations/${conversationId}/message`,
        {
          method: "POST",
          headers,
          body: JSON.stringify({ content }),
        },
      )

      if (!response.ok) {
        throw new Error("Failed to send message")
      }

      // Parse SSE response
      const reader = response.body?.getReader()
      const decoder = new TextDecoder()

      if (reader) {
        let buffer = ""
        while (true) {
          const { done, value } = await reader.read()
          if (done) break
          buffer += decoder.decode(value, { stream: true })

          // Parse SSE events
          const lines = buffer.split("\n")
          buffer = lines.pop() || ""

          for (const line of lines) {
            if (line.startsWith("data: ")) {
              const data = JSON.parse(line.slice(6))
              if (data.type === "message") {
                setStreamingMessage(data.content)
              } else if (data.type === "error") {
                throw new Error(data.error)
              }
            }
          }
        }
      }

      setStreamingMessage(null)
      queryClient.invalidateQueries({
        queryKey: ["courses", courseId, "conversations", conversationId],
      })
      queryClient.invalidateQueries({
        queryKey: ["courses", courseId, "conversations"],
      })
    },
  })

  const handleSubmit = (e: React.FormEvent) => {
    e.preventDefault()
    if (!input.trim() || sendMutation.isPending) return
    const msg = input
    setInput("")
    sendMutation.mutate(msg)
  }

  return (
    <>
      <ScrollArea className="flex-1 pr-4">
        <div className="space-y-4 py-4">
          {isLoading && (
            <p className="text-muted-foreground text-center">Loading...</p>
          )}
          {messages?.map((msg) => (
            <ChatBubble key={msg.id} message={msg} />
          ))}
          {sendMutation.isPending && streamingMessage === "" && (
            <div className="flex justify-start">
              <div className="bg-muted rounded-lg px-4 py-2 max-w-[80%]">
                <p className="text-sm text-muted-foreground">Thinking...</p>
              </div>
            </div>
          )}
          {streamingMessage && (
            <div className="flex justify-start">
              <div className="bg-muted rounded-lg px-4 py-2 max-w-[80%]">
                <p className="text-sm whitespace-pre-wrap">{streamingMessage}</p>
              </div>
            </div>
          )}
          <div ref={messagesEndRef} />
        </div>
      </ScrollArea>

      <form onSubmit={handleSubmit} className="flex gap-2 pt-4 border-t">
        <Input
          value={input}
          onChange={(e) => setInput(e.target.value)}
          placeholder="Ask about the course materials..."
          disabled={sendMutation.isPending}
          className="flex-1"
        />
        <Button type="submit" disabled={sendMutation.isPending || !input.trim()}>
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
          isUser
            ? "bg-primary text-primary-foreground"
            : "bg-muted"
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
