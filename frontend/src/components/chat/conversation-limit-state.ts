import type { ConversationTokenState } from "@/lib/types"

/**
 * Where a conversation sits against its course's token ceilings.
 *
 *   * `ok`      ; below the nudge threshold, nothing rendered.
 *   * `nudge`   ; past the soft limit. Dismissible; the composer stays live.
 *   * `blocked` ; past the hard limit. The composer is gone and the only
 *                 way forward is a new conversation.
 */
export type ConversationLimitState = "ok" | "nudge" | "blocked"

/**
 * Resolve the display state from the server-computed token state.
 * `0` disables a ceiling, matching the course columns and the spend-cap
 * convention. Kept in its own module (rather than next to the component)
 * so it is directly unit-testable: this is the one rule the nudge and the
 * block both key off, and it has to agree with the backend's
 * `ConversationTokenState::is_over_hard_limit`.
 */
export function conversationLimitState(
  token: ConversationTokenState | undefined,
): ConversationLimitState {
  if (!token) return "ok"
  if (token.hard_limit > 0 && token.total >= token.hard_limit) return "blocked"
  if (token.soft_limit > 0 && token.total >= token.soft_limit) return "nudge"
  return "ok"
}
