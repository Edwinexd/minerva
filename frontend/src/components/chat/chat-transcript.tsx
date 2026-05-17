/**
 * Shared transcript view: skeletons → message bubbles → optional
 * pending user message → streaming assistant bubble → error line.
 *
 * Owns the scroll-to-bottom behaviour. The two callers (regular chat
 * page, embed iframe) feed it the same shape of data via the
 * `useChatStream` hook plus their own message list.
 */
import React, { useCallback, useEffect, useRef } from "react"

import { Skeleton } from "@/components/ui/skeleton"

import {
  ChatBubble,
  type ChatBubbleLabels,
  type ChatBubbleMessage,
  MarkdownContent,
} from "./chat-bubble"
import { ThinkingBlock, type ThinkingBlockLabels } from "./thinking-block"
import type { ToolEvent } from "./use-chat-stream"

export interface ChatTranscriptProps<M extends ChatBubbleMessage> {
  messages: M[] | undefined
  isLoading: boolean
  /** The user message we have already shown but the server has not echoed back yet. */
  pendingUserMsg: string | null
  /** True while we are receiving SSE tokens. */
  streaming: boolean
  /** Tokens streamed so far (rendered as in-progress markdown). */
  streamedTokens: string
  error: string | null
  /**
   * Concatenated `thinking_token` SSE stream (research phase
   * tokens). Only populated for `tool_use_enabled` courses;
   * legacy strategies pass an empty string.
   */
  thinkingTokens?: string
  /** Tool-call events emitted during the research phase. */
  toolEvents?: ToolEvent[]
  /** True while research phase is active. */
  thinkingActive?: boolean
  bubbleLabels: ChatBubbleLabels
  /** Labels for the collapsible "Thinking" disclosure. */
  thinkingLabels?: ThinkingBlockLabels
  /** aria-label for the in-progress assistant bubble. */
  assistantResponseLabel: string
  /** Optional per-message feedback slot (thumbs up/down on the regular chat). */
  renderFeedbackSlot?: (msg: M) => React.ReactNode
  /** Optional content rendered immediately after a specific message (e.g. teacher notes). */
  renderAfterMessage?: (msg: M) => React.ReactNode
  /** Optional content rendered before the message list (e.g. conversation-level notes). */
  renderBeforeMessages?: () => React.ReactNode
}

export function ChatTranscript<M extends ChatBubbleMessage>({
  messages,
  isLoading,
  pendingUserMsg,
  streaming,
  streamedTokens,
  error,
  thinkingTokens,
  toolEvents,
  thinkingActive,
  bubbleLabels,
  thinkingLabels,
  assistantResponseLabel,
  renderFeedbackSlot,
  renderAfterMessage,
  renderBeforeMessages,
}: ChatTranscriptProps<M>) {
  const messagesEndRef = useRef<HTMLDivElement>(null)
  const scrollToBottom = useCallback(() => {
    messagesEndRef.current?.scrollIntoView({ behavior: "smooth" })
  }, [])

  useEffect(() => {
    scrollToBottom()
  }, [messages, streamedTokens, scrollToBottom])

  // Identify the most-recent assistant message so we can attach the
  // post-stream "Thinking" disclosure to it. When streaming wraps
  // up, the in-progress bubble disappears and the persisted
  // assistant message takes its place; the thinking buffer in state
  // outlives the streaming bubble, so the disclosure stays
  // expandable on the just-arrived answer until the user sends a
  // new message (which calls `reset()` on the stream hook).
  const lastAssistantId =
    !streaming && (thinkingTokens || (toolEvents && toolEvents.length > 0))
      ? messages
          ?.slice()
          .reverse()
          .find((m) => m.role === "assistant")?.id
      : undefined

  return (
    <div className="space-y-4 py-4">
      {renderBeforeMessages?.()}
      {isLoading &&
        Array.from({ length: 3 }).map((_, i) => (
          <div
            key={i}
            className={`flex ${i % 2 === 0 ? "justify-end" : "justify-start"}`}
          >
            <Skeleton className="h-12 w-64 rounded-lg" />
          </div>
        ))}
      {messages?.map((msg) => (
        <React.Fragment key={msg.id}>
          <ChatBubble
            message={msg}
            labels={bubbleLabels}
            feedbackSlot={renderFeedbackSlot?.(msg)}
          />
          {msg.id === lastAssistantId && thinkingLabels && (
            <div className="flex justify-start">
              <div className="max-w-[80%] w-full">
                <ThinkingBlock
                  thinkingTokens={thinkingTokens || ""}
                  toolEvents={toolEvents || []}
                  active={false}
                  labels={thinkingLabels}
                />
              </div>
            </div>
          )}
          {renderAfterMessage?.(msg)}
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
            className="bg-muted rounded-lg px-4 py-2 max-w-[80%] space-y-2"
            aria-live="polite"
            aria-atomic="false"
            aria-label={assistantResponseLabel}
          >
            {(thinkingTokens || (toolEvents && toolEvents.length > 0)) &&
              thinkingLabels && (
                <ThinkingBlock
                  thinkingTokens={thinkingTokens || ""}
                  toolEvents={toolEvents || []}
                  active={!!thinkingActive}
                  labels={thinkingLabels}
                />
              )}
            {streamedTokens ? (
              <MarkdownContent content={streamedTokens} />
            ) : thinkingActive ? null : (
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
        <p role="alert" className="text-sm text-destructive text-center">
          {error}
        </p>
      )}
      <div ref={messagesEndRef} />
    </div>
  )
}
