import { describe, expect, it } from "vitest"
import { conversationLimitState } from "@/components/chat/conversation-limit-state"

/**
 * These assertions mirror `chat::ConversationTokenState::is_over_hard_limit`
 * on the backend. If the two ever disagree the student sees a live
 * composer that the send endpoint then rejects with a 409, so the
 * boundary cases are pinned here deliberately.
 */
describe("conversationLimitState", () => {
  const state = (total: number, soft: number, hard: number) => ({
    total,
    soft_limit: soft,
    hard_limit: hard,
  })

  it("is ok below the nudge threshold", () => {
    expect(conversationLimitState(state(299_999, 300_000, 1_000_000))).toBe("ok")
  })

  it("nudges exactly at the soft limit", () => {
    // Backend compares with >=, so the boundary token counts as crossed.
    expect(conversationLimitState(state(300_000, 300_000, 1_000_000))).toBe(
      "nudge",
    )
  })

  it("blocks exactly at the hard limit", () => {
    expect(conversationLimitState(state(1_000_000, 300_000, 1_000_000))).toBe(
      "blocked",
    )
  })

  it("blocked wins over nudge once both are crossed", () => {
    expect(conversationLimitState(state(2_000_000, 300_000, 1_000_000))).toBe(
      "blocked",
    )
  })

  it("treats 0 as disabled per limit, matching the course columns", () => {
    // Nudge off, ceiling on.
    expect(conversationLimitState(state(500_000, 0, 1_000_000))).toBe("ok")
    expect(conversationLimitState(state(1_000_000, 0, 1_000_000))).toBe(
      "blocked",
    )
    // Ceiling off, nudge on: a long thread nags but is never closed.
    expect(conversationLimitState(state(9_000_000, 300_000, 0))).toBe("nudge")
    // Both off.
    expect(conversationLimitState(state(9_000_000, 0, 0))).toBe("ok")
  })

  it("is ok when the server sent no token state", () => {
    expect(conversationLimitState(undefined)).toBe("ok")
  })

  describe("topic switch", () => {
    it("surfaces the topic state on a short conversation", () => {
      expect(conversationLimitState(state(1000, 300_000, 1_000_000), true)).toBe(
        "topic",
      )
    })

    it("is ok when no switch was confirmed", () => {
      expect(
        conversationLimitState(state(1000, 300_000, 1_000_000), false),
      ).toBe("ok")
    })

    it("yields to the length states when both apply", () => {
      // Both ask for the same action, so saying it twice adds nothing;
      // the length framing also explains why the chat may stop
      // accepting messages, so it wins.
      expect(
        conversationLimitState(state(400_000, 300_000, 1_000_000), true),
      ).toBe("nudge")
      expect(
        conversationLimitState(state(2_000_000, 300_000, 1_000_000), true),
      ).toBe("blocked")
    })

    it("applies before any token state has loaded", () => {
      expect(conversationLimitState(undefined, true)).toBe("topic")
    })
  })
})
