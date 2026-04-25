/**
 * Shared chat-message bubble used by the regular Shibboleth chat page
 * and the LTI embed iframe.
 *
 * The two callers differ in:
 *   - i18n namespace ("student" vs "auth")
 *   - whether the assistant footer shows usage stats and feedback controls
 *
 * Both branches were copied/pasted variants of each other before this
 * was extracted; the embed copy was missing thumbs-up/down support and
 * teacher-pinned-conversation handling. Centralising here means future
 * source/feedback tweaks land in both places automatically.
 */
import React, { useState } from "react"
import { ChevronDown, ChevronUp } from "lucide-react"
import Markdown from "react-markdown"
import remarkGfm from "remark-gfm"

export interface ChatBubbleMessage {
  id: string
  role: "user" | "assistant"
  content: string
  chunks_used: string[] | null
  tokens_prompt?: number | null
  tokens_completion?: number | null
  generation_ms?: number | null
  retrieval_count?: number | null
}

/**
 * Strings the bubble needs. Passed in rather than read via
 * `useTranslation` because the two call sites use different i18n
 * namespaces ("student" vs the embed-only "auth.embed").
 */
export interface ChatBubbleLabels {
  /** Text on the source toggle button, e.g. "3 sources". */
  sourceCount: (count: number) => string
  /** Fallback label when a chunk has no `[Source: ...]` prefix. */
  unknownSource: string
  /** Shown when a source has only a header and no body text. */
  sourceUnavailable: string
  /**
   * Optional usage-stats labels. When omitted the footer hides token
   * counts, generation time, "using" suffix, and the across-retrievals
   * note (which is the embed view's behaviour).
   */
  stats?: {
    tokensUsed: (count: number) => string
    generationTime: (seconds: string) => string
    usingSuffix: string
    acrossRetrievals: (count: number) => string
  }
}

export function MarkdownContent({
  content,
  className,
}: {
  content: string
  className?: string
}) {
  return (
    <div className={`prose prose-sm dark:prose-invert max-w-none ${className || ""}`}>
      <Markdown remarkPlugins={[remarkGfm]}>{content}</Markdown>
    </div>
  )
}

export function ChatBubble({
  message,
  labels,
  feedbackSlot = null,
}: {
  message: ChatBubbleMessage
  labels: ChatBubbleLabels
  /** Rendered inside the assistant footer (e.g. <FeedbackControls/>). */
  feedbackSlot?: React.ReactNode
}) {
  const isUser = message.role === "user"
  const [showSources, setShowSources] = useState(false)
  const chunks = message.chunks_used

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
            {labels.stats && message.tokens_prompt != null && (
              <span>
                {labels.stats.tokensUsed(
                  message.tokens_prompt + (message.tokens_completion ?? 0),
                )}
                {message.generation_ms != null &&
                  labels.stats.generationTime(
                    (message.generation_ms / 1000).toFixed(1),
                  )}
                {chunks && chunks.length > 0 && labels.stats.usingSuffix}
              </span>
            )}
            {chunks && chunks.length > 0 && (
              <>
                <button
                  className="underline hover:text-foreground"
                  onClick={() => setShowSources(!showSources)}
                >
                  {labels.sourceCount(chunks.length)}
                  {showSources ? (
                    <ChevronUp className="inline w-3 h-3 ml-0.5" />
                  ) : (
                    <ChevronDown className="inline w-3 h-3 ml-0.5" />
                  )}
                </button>
                {labels.stats &&
                  message.retrieval_count != null &&
                  message.retrieval_count > 1 && (
                    <span>{labels.stats.acrossRetrievals(message.retrieval_count)}</span>
                  )}
              </>
            )}
            {feedbackSlot}
          </div>
        )}
        {showSources && chunks && (
          <div className="mt-2 space-y-2 border-t pt-2">
            {chunks.map((chunk, i) => {
              const sourceMatch = chunk.match(/^\[Source: (.+?)\](\n|$)/)
              const source = sourceMatch ? sourceMatch[1] : labels.unknownSource
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
                      {labels.sourceUnavailable}
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
