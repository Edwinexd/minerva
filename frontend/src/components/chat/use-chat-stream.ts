/**
 * Encapsulates the SSE streaming protocol used by both the regular and
 * embed chat send-message endpoints:
 *
 *   POST .../message  →  text/event-stream of `data: {"type":"token","token":"..."}`
 *                        terminated by either an `error` event or stream EOF.
 *
 * The caller supplies the actual `fetch` invocation (the URL and body
 * shape differ between the two routes; the regular route takes the
 * token via cookie, the embed route ships it in the request body).
 *
 * Returns a snapshot of the stream state plus `send` / `reset` actions
 * so the calling component can drive a `<ChatTranscript>`.
 */
import { useState } from "react"

export interface ChatStreamState {
  streaming: boolean
  streamedTokens: string
  pendingUserMsg: string | null
  error: string | null
  /**
   * Concatenation of `thinking_token` SSE events emitted by the
   * backend's research phase (active only when the course has
   * `tool_use_enabled = true`). When the writeup phase starts the
   * backend emits a `thinking_done` event; the frontend keeps this
   * buffer around so the user can still expand the "Thinking"
   * disclosure under the assistant reply.
   */
  thinkingTokens: string
  /**
   * Tool-use events surfaced for the "Thinking" disclosure. Each
   * entry pairs a `tool_call` with its later `tool_result` (matched
   * positionally; the backend emits them in pair-order per turn).
   */
  toolEvents: ToolEvent[]
  /**
   * True between `thinking_started` (implicit at the first
   * thinking_token) and `thinking_done`. Lets the UI dim the chat
   * answer area until writeup tokens start flowing.
   */
  thinkingActive: boolean
  /**
   * Wall-clock duration of the research phase in milliseconds.
   * `null` until `thinking_done` arrives with its `duration_ms`
   * field (or until the user picks an older message from history
   * whose persisted `thinking_ms` populates it via the bubble).
   * Used to render "Thought for Ns" on the disclosure.
   */
  thinkingDurationMs: number | null
}

export interface ToolEvent {
  name: string
  args?: unknown
  resultSummary?: string
  /**
   * Raw JSON payload the tool returned to the model (truncated to
   * `MAX_TOOL_RESULT_BYTES` server-side). Rendered click-to-expand
   * so a curious user can see exactly what came back; `undefined`
   * until the matching `tool_result` SSE event arrives, which is
   * the only producer.
   */
  result?: unknown
}

/**
 * Hook off non-token/non-error SSE events. The chat protocol emits
 * `{type: "conversation_created", id}` when the server lazily creates
 * a conversation row in response to the first message; callers use
 * this to learn the new id and navigate to it.
 */
export type ChatStreamEvent = { type: string; [k: string]: unknown }

export interface ChatStreamActions {
  /**
   * Optimistically display `content` as the user message, kick off the
   * server fetch via `doFetch`, and stream tokens into `streamedTokens`
   * until the response ends. Resolves to `true` on success, `false` if
   * the server emitted an `error` event or the fetch threw.
   *
   * `onEvent` (optional) is called for every `data:` event that is
   * neither a `token` nor an `error`; the hook stays unaware of
   * domain-specific events like `conversation_created`.
   */
  send: (
    content: string,
    doFetch: () => Promise<Response>,
    onEvent?: (data: ChatStreamEvent) => void,
  ) => Promise<boolean>
  /** Wipe state when switching conversations. */
  reset: () => void
  /** Lets the caller surface its own errors (e.g. conversation create failed). */
  setError: (e: string | null) => void
  /** Lets the caller seed an optimistic user message before send (e.g. while creating a conversation). */
  setPendingUserMsg: (s: string | null) => void
}

export function useChatStream(
  unknownErrorLabel: string,
): ChatStreamState & ChatStreamActions {
  const [streaming, setStreaming] = useState(false)
  const [streamedTokens, setStreamedTokens] = useState("")
  const [pendingUserMsg, setPendingUserMsg] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [thinkingTokens, setThinkingTokens] = useState("")
  const [toolEvents, setToolEvents] = useState<ToolEvent[]>([])
  const [thinkingActive, setThinkingActive] = useState(false)
  const [thinkingDurationMs, setThinkingDurationMs] = useState<number | null>(
    null,
  )

  const reset = () => {
    setStreaming(false)
    setStreamedTokens("")
    setPendingUserMsg(null)
    setError(null)
    setThinkingTokens("")
    setToolEvents([])
    setThinkingActive(false)
    setThinkingDurationMs(null)
  }

  const send = async (
    content: string,
    doFetch: () => Promise<Response>,
    onEvent?: (data: ChatStreamEvent) => void,
  ): Promise<boolean> => {
    setError(null)
    setStreaming(true)
    setStreamedTokens("")
    setPendingUserMsg(content)
    setThinkingTokens("")
    setToolEvents([])
    setThinkingActive(false)
    setThinkingDurationMs(null)

    let success = true
    try {
      const response = await doFetch()
      if (!response.ok) {
        // Backend error responses are `{ code, message, params }`
        // (see `minerva-server/src/error.rs`). Prefer `message`,
        // fall back to `error` for older shapes, then statusText.
        // Surfacing the real message matters for things like the
        // 429 quota cap so the student sees "daily token quota
        // exceeded" rather than the bare HTTP statusText.
        const body = await response.json().catch(() => ({}))
        throw new Error(body.message || body.error || response.statusText)
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
                  // Writeup-phase token (always; legacy strategies
                  // emit these directly without a thinking phase).
                  // First `token` after thinking implicitly closes
                  // the thinking-active flag, in case the backend
                  // didn't emit a `thinking_done` for some reason.
                  setThinkingActive(false)
                  setStreamedTokens((prev) => prev + data.token)
                } else if (data.type === "thinking_token") {
                  // Research-phase token (tool_use_enabled courses
                  // only). Routed to a separate buffer so the UI
                  // can render a collapsible "Thinking" disclosure
                  // distinct from the answer.
                  setThinkingActive(true)
                  if (typeof data.token === "string") {
                    setThinkingTokens((prev) => prev + data.token)
                  }
                } else if (data.type === "tool_call") {
                  // Model issued a tool call. Append a new pair to
                  // `toolEvents`; the matching tool_result event
                  // patches the last entry below.
                  setToolEvents((prev) => [
                    ...prev,
                    {
                      name: typeof data.name === "string" ? data.name : "?",
                      args: data.args,
                    },
                  ])
                } else if (data.type === "tool_result") {
                  // Match positionally to the most recent un-
                  // resolved tool_call (the backend emits them in
                  // pair-order within one turn).
                  setToolEvents((prev) => {
                    const next = [...prev]
                    for (let i = next.length - 1; i >= 0; i--) {
                      if (next[i].resultSummary === undefined) {
                        next[i] = {
                          ...next[i],
                          resultSummary:
                            typeof data.result_summary === "string"
                              ? data.result_summary
                              : undefined,
                          result: data.result,
                        }
                        break
                      }
                    }
                    return next
                  })
                } else if (data.type === "thinking_done") {
                  setThinkingActive(false)
                  if (typeof data.duration_ms === "number") {
                    setThinkingDurationMs(data.duration_ms)
                  }
                } else if (data.type === "error") {
                  setError(data.error)
                  success = false
                } else if (data.type === "rewrite") {
                  // Extraction-guard intercept: the backend
                  // streamed an original answer, then decided
                  // post-generation to swap it for a Socratic
                  // rewrite. Replace the in-flight token buffer
                  // so the student sees the rewrite immediately,
                  // rather than continuing to see the original
                  // until the conversation query refetches the
                  // persisted (rewritten) message.
                  if (typeof data.content === "string") {
                    setStreamedTokens(data.content)
                  }
                  if (onEvent) onEvent(data)
                } else if (onEvent) {
                  onEvent(data)
                }
              } catch {
                // Skip malformed JSON.
              }
            }
          }
        }
      }
    } catch (e) {
      setError(e instanceof Error ? e.message : unknownErrorLabel)
      success = false
    } finally {
      setStreaming(false)
      setStreamedTokens("")
      setThinkingActive(false)
      // Keep `thinkingTokens` and `toolEvents` populated after the
      // stream ends so the assistant message's "Thinking" disclosure
      // stays expandable until the user navigates away or sends
      // another message (which calls reset() at the top of send).
    }
    // Clear the optimistic user echo only on success. On failure
    // (e.g. 429 quota cap, fetch threw, or a mid-stream `error`
    // event) we keep it visible alongside the error so the student
    // sees what they sent. Critically, on the `/new` route this
    // also keeps `showGreeting` in chat-page.tsx false, preventing
    // a one-frame flash back to the empty-state hero. The next
    // send() call overwrites pendingUserMsg at the top, and
    // reset() (called on conversation switch) clears it explicitly.
    if (success) setPendingUserMsg(null)
    return success
  }

  return {
    streaming,
    streamedTokens,
    pendingUserMsg,
    error,
    thinkingTokens,
    toolEvents,
    thinkingActive,
    thinkingDurationMs,
    send,
    reset,
    setError,
    setPendingUserMsg,
  }
}
