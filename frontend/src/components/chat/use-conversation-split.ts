import { useCallback, useState } from "react"
import { useApiErrorMessage } from "@/lib/use-api-error"
import type { ConversationContinuation } from "@/lib/types"
import type { LimitNoticeKind } from "./conversation-limit-state"

/**
 * sessionStorage key holding `<conversation id>:<kind>` entries for the
 * banners the student has dismissed.
 *
 * Dismissal has to outlive the component: the student can navigate to
 * another conversation and back, and a banner that reappears every time
 * is precisely the kind students learn to tune out. It deliberately
 * does NOT outlive the browser session, so the nudge gets one more
 * chance the next time they sit down with the same thread. The blocked
 * state ignores this entirely; it is not dismissible.
 *
 * Tracked per kind so that waving off a topic-switch nudge at turn
 * three does not also silence the length nudge that arrives later.
 */
const DISMISSED_KEY = "minerva-conversation-limit-dismissed"

const KINDS: readonly LimitNoticeKind[] = ["length", "topic"]

const entry = (conversationId: string, kind: LimitNoticeKind) =>
  `${conversationId}:${kind}`

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

function writeDismissed(entries: string[]) {
  try {
    sessionStorage.setItem(DISMISSED_KEY, JSON.stringify(entries))
  } catch {
    // Best-effort; dismissal just won't persist across navigation.
  }
}

export interface ConversationSplit {
  /** Mutation in flight. Disables the action button. */
  pending: boolean
  /** Translated failure text, or null. */
  error: string | null
  /** Banner kinds the student dismissed for this conversation. */
  dismissed: ReadonlySet<LimitNoticeKind>
  /**
   * Mint the continuation and move there. `draft` is composer text to
   * carry into the new conversation's composer.
   */
  run: (draft?: string) => void
  dismiss: (kinds: readonly LimitNoticeKind[]) => void
}

/**
 * Owns the "continue this conversation in a new one" action and the
 * dismissal state for its nudges.
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
  onSplit: (created: ConversationContinuation, draft?: string) => void
}): ConversationSplit {
  const formatError = useApiErrorMessage()
  const [pending, setPending] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [dismissedEntries, setDismissedEntries] = useState<string[]>(readDismissed)

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

  const run = useCallback(
    (draft?: string) => {
      if (!conversationId || pending) return
      setPending(true)
      setError(null)
      doSplit(conversationId)
        .then((created) => onSplit(created, draft))
        .catch((e) => setError(formatError(e)))
        .finally(() => setPending(false))
    },
    // `formatError` and `onSplit` are rebuilt per render by their
    // callers; depending on them would rebuild this callback every
    // render for no benefit.
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [conversationId, pending, doSplit],
  )

  const dismiss = useCallback(
    (kinds: readonly LimitNoticeKind[]) => {
      if (!conversationId) return
      setDismissedEntries((prev) => {
        const added = kinds
          .map((kind) => entry(conversationId, kind))
          .filter((e) => !prev.includes(e))
        if (added.length === 0) return prev
        const next = [...prev, ...added]
        writeDismissed(next)
        return next
      })
    },
    [conversationId],
  )

  const dismissed = new Set(
    conversationId === null
      ? []
      : KINDS.filter((kind) =>
          dismissedEntries.includes(entry(conversationId, kind)),
        ),
  )

  return { pending, error, dismissed, run, dismiss }
}
