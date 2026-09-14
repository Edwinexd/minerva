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

/**
 * What a banner's button does, named after the endpoint it calls:
 *
 *   * `branch`   ; carry the new-topic exchange into a fresh chat, verbatim.
 *   * `continue` ; carry an LLM recap of the whole thread into a fresh chat.
 */
export type ConversationLimitAction = "branch" | "continue"

/** The two dismissible banner families, dismissed independently. */
export type LimitNoticeKind = "length" | "topic"

/**
 * A pending topic switch means the student is changing subject, so
 * carrying the new question beats recapping the thread they are
 * leaving, whichever banner is up. Only the length ceiling with no
 * switch in sight falls back to the recap.
 */
export function conversationLimitAction(
  topicSwitch: boolean | undefined,
): ConversationLimitAction {
  return topicSwitch ? "branch" : "continue"
}

export interface ConversationLimitView {
  state: ConversationLimitState
  action: ConversationLimitAction
  /** Kinds that dismissing the visible banner silences. */
  dismisses: LimitNoticeKind[]
}

/**
 * The banner a surface should render once the student's dismissals are
 * applied.
 *
 * Dismissing a banner that carries both signals (long AND off-topic)
 * silences both; otherwise the topic banner would pop up in its place
 * the moment the student waved the length one away. A length nudge
 * dismissed earlier, before any switch, still lets a later topic
 * banner through, since that is new information.
 */
export function resolveConversationLimit(
  token: ConversationTokenState | undefined,
  topicSwitch = false,
  dismissed: ReadonlySet<LimitNoticeKind> = new Set(),
): ConversationLimitView {
  const action = conversationLimitAction(topicSwitch)
  const raw = conversationLimitState(token, topicSwitch)
  if (raw === "blocked") return { state: raw, action, dismisses: [] }
  if (raw === "nudge" && !dismissed.has("length")) {
    return {
      state: raw,
      action,
      dismisses: topicSwitch ? ["length", "topic"] : ["length"],
    }
  }
  if (topicSwitch && !dismissed.has("topic")) {
    return { state: "topic", action, dismisses: ["topic"] }
  }
  return { state: "ok", action, dismisses: [] }
}
