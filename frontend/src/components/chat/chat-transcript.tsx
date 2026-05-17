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

/**
 * Optional persisted-thinking shape that callers can attach to each
 * `ChatBubbleMessage`. Mirrors `PersistedToolEvent` from
 * `lib/types`; ChatTranscript reads these to render the
 * post-refresh "Thinking" disclosure ABOVE each assistant message,
 * matching where the live disclosure sits during streaming.
 */
export interface PersistedThinking {
  thinking_transcript: string | null
  tool_events: ToolEvent[] | null
}

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
  /**
   * Pull the persisted research-phase fields off a message. Returns
   * `null` when the message has no thinking attached (legacy
   * single-pass messages). When omitted, no historical disclosure
   * renders; the live streaming one still works because it reads
   * from the `thinkingTokens` / `toolEvents` props above.
   */
  getPersistedThinking?: (msg: M) => PersistedThinking | null
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
  getPersistedThinking,
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
      {messages?.map((msg) => {
        // Persisted research-phase data for this specific assistant
        // message, if any. Rendered ABOVE the bubble so it sits in
        // the same visual position the live disclosure occupies
        // during streaming (above the answer being written).
        const persisted =
          msg.role === "assistant" && getPersistedThinking
            ? getPersistedThinking(msg)
            : null
        const hasPersistedThinking =
          persisted &&
          ((persisted.thinking_transcript &&
            persisted.thinking_transcript.length > 0) ||
            (persisted.tool_events && persisted.tool_events.length > 0))
        return (
          <React.Fragment key={msg.id}>
            {hasPersistedThinking && thinkingLabels && (
              <div className="flex justify-start">
                <div className="max-w-[80%] w-full">
                  <ThinkingBlock
                    thinkingTokens={persisted?.thinking_transcript || ""}
                    toolEvents={persisted?.tool_events || []}
                    active={false}
                    labels={thinkingLabels}
                  />
                </div>
              </div>
            )}
            <ChatBubble
              message={msg}
              labels={bubbleLabels}
              feedbackSlot={renderFeedbackSlot?.(msg)}
            />
            {renderAfterMessage?.(msg)}
          </React.Fragment>
        )
      })}
      {pendingUserMsg && (
        <div className="flex justify-end">
          <div className="rounded-lg px-4 py-2 max-w-[80%] bg-primary text-primary-foreground">
            <p className="text-sm whitespace-pre-wrap">{pendingUserMsg}</p>
          </div>
        </div>
      )}
      {streaming && (
        <>
          {(thinkingTokens || (toolEvents && toolEvents.length > 0)) &&
            thinkingLabels && (
              <div className="flex justify-start">
                <div className="max-w-[80%] w-full">
                  <ThinkingBlock
                    thinkingTokens={thinkingTokens || ""}
                    toolEvents={toolEvents || []}
                    active={!!thinkingActive}
                    labels={thinkingLabels}
                  />
                </div>
              </div>
            )}
          <div className="flex justify-start">
            <div
              className="bg-muted rounded-lg px-4 py-2 max-w-[80%]"
              aria-live="polite"
              aria-atomic="false"
              aria-label={assistantResponseLabel}
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
        </>
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
