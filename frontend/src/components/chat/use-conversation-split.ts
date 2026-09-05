import { useCallback, useState } from "react"
import { useApiErrorMessage } from "@/lib/use-api-error"
import type { ConversationContinuation } from "@/lib/types"

/**
 * sessionStorage key holding the conversation ids whose nudge the
 * student has dismissed.
 *
 * Dismissal has to outlive the component: the student can navigate to
 * another conversation and back, and a banner that reappears every time
 * is precisely the kind students learn to tune out. It deliberately
 * does NOT outlive the browser session, so the nudge gets one more
 * chance the next time they sit down with the same thread. The blocked
 * state ignores this entirely; it is not dismissible.
 */
const DISMISSED_KEY = "minerva-conversation-limit-dismissed"

function readDismissed(): string[] {
  try {
    const raw = sessionStorage.getItem(DISMISSED_KEY)
    const parsed = raw ? JSON.parse(raw) : []
    return Array.isArray(parsed)
      ? parsed.filter((v) => typeof v === "string")
      : []
  } catch {
    // Private-mode / quota / malformed JSON: fall back to "nothing
    // dismissed". Showing the nudge once too often is a better failure
    // than crashing the chat page.
    return []
  }
}

function writeDismissed(ids: string[]) {
  try {
    sessionStorage.setItem(DISMISSED_KEY, JSON.stringify(ids))
  } catch {
    // Best-effort; dismissal just won't persist across navigation.
  }
}

export interface ConversationSplit {
  /** Mutation in flight. Disables the action button. */
  pending: boolean
  /** Translated failure text, or null. */
  error: string | null
  /** True when the student dismissed this conversation's nudge. */
  dismissed: boolean
  run: () => void
  dismiss: () => void
}

/**
 * Owns the "continue this conversation in a new one" action and the
 * dismissal state for its nudge.
 *
 * Surface-agnostic: the caller supplies `doSplit`, because the
 * Shibboleth route goes through `lib/api` (cookie auth) while the embed
 * route has to append its signed token to the query string.
 */
export function useConversationSplit({
  conversationId,
  doSplit,
  onSplit,
}: {
  conversationId: string | null
  doSplit: (conversationId: string) => Promise<ConversationContinuation>
  onSplit: (created: ConversationContinuation) => void
}): ConversationSplit {
  const formatError = useApiErrorMessage()
  const [pending, setPending] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [dismissedIds, setDismissedIds] = useState<string[]>(readDismissed)

  // A stale error from a previous conversation must not render over a
  // different thread's banner. Cleared during render rather than in an
  // effect so React batches it with the parent's render instead of
  // triggering a second pass (same pattern as `ChatPage`'s sidebar
  // reset).
  const [prevConversationId, setPrevConversationId] = useState(conversationId)
  if (prevConversationId !== conversationId) {
    setPrevConversationId(conversationId)
    setError(null)
  }

  const run = useCallback(() => {
    if (!conversationId || pending) return
    setPending(true)
    setError(null)
    doSplit(conversationId)
      .then(onSplit)
      .catch((e) => setError(formatError(e)))
      .finally(() => setPending(false))
    // `formatError` and `onSplit` are rebuilt per render by their
    // callers; depending on them would rebuild this callback every
    // render for no benefit.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [conversationId, pending, doSplit])

  const dismiss = useCallback(() => {
    if (!conversationId) return
    setDismissedIds((prev) => {
      if (prev.includes(conversationId)) return prev
      const next = [...prev, conversationId]
      writeDismissed(next)
      return next
    })
  }, [conversationId])

  return {
    pending,
    error,
    dismissed: conversationId !== null && dismissedIds.includes(conversationId),
    run,
    dismiss,
  }
}
