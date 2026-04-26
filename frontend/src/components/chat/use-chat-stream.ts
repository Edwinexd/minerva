/**
 * Encapsulates the SSE streaming protocol used by both the regular and
 * embed chat send-message endpoints:
 *
 *   POST .../message  →  text/event-stream of `data: {"type":"token","token":"..."}`
 *                        terminated by either an `error` event or stream EOF.
 *
 * The caller supplies the actual `fetch` invocation (the URL and body
 * shape differ between the two routes -- the regular route takes the
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
   * neither a `token` nor an `error` -- the hook stays unaware of
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

  const reset = () => {
    setStreaming(false)
    setStreamedTokens("")
    setPendingUserMsg(null)
    setError(null)
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

    let success = true
    try {
      const response = await doFetch()
      if (!response.ok) {
        const body = await response
          .json()
          .catch(() => ({ error: response.statusText }))
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
      setPendingUserMsg(null)
    }
    return success
  }

  return {
    streaming,
    streamedTokens,
    pendingUserMsg,
    error,
    send,
    reset,
    setError,
    setPendingUserMsg,
  }
}
