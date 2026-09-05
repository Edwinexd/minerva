import type { ConversationTokenState } from "@/lib/types"

/**
 * Where a conversation sits against its course's token ceilings.
 *
 *   * `ok`      ; below the nudge threshold, nothing rendered.
 *   * `nudge`   ; past the soft limit. Dismissible; the composer stays live.
 *   * `blocked` ; past the hard limit. The composer is gone and the only
 *                 way forward is a new conversation.
 */
export type ConversationLimitState = "ok" | "topic" | "nudge" | "blocked"

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
  topicSwitch = false,
): ConversationLimitState {
  if (!token) return topicSwitch ? "topic" : "ok"
  if (token.hard_limit > 0 && token.total >= token.hard_limit) return "blocked"
  if (token.soft_limit > 0 && token.total >= token.soft_limit) return "nudge"
  // Ranked below the spend states on purpose. Both ask for the same
  // action, so when a conversation is long AND has switched topic there
  // is no value in saying so twice; the length framing is the one that
  // also explains why the chat may stop accepting messages.
  return topicSwitch ? "topic" : "ok"
}
