import type { RefObject } from "react"
import { render } from "@testing-library/react"
import { describe, expect, it, vi } from "vitest"

import { ChatTranscript } from "@/components/chat/chat-transcript"
import type {
  ChatBubbleLabels,
  ChatBubbleMessage,
} from "@/components/chat/chat-bubble"

const bubbleLabels: ChatBubbleLabels = {
  sourceCount: (count) => `${count} sources`,
  unknownSource: "Unknown source",
  sourceUnavailable: "Source unavailable",
}

describe("ChatTranscript", () => {
  it("scrolls only its transcript container when a user sends a message", () => {
    const scrollTo = vi.fn()
    const scrollIntoView = vi.spyOn(Element.prototype, "scrollIntoView")
    const scrollContainerRef = {
      current: { scrollHeight: 864, scrollTo },
    } as unknown as RefObject<HTMLElement | null>
    const props = {
      scrollContainerRef,
      messages: [] as ChatBubbleMessage[],
      isLoading: false,
      streaming: false,
      streamedTokens: "",
      error: null,
      bubbleLabels,
      assistantResponseLabel: "Assistant response",
    }

    const { rerender } = render(
      <ChatTranscript {...props} pendingUserMsg={null} />,
    )
    rerender(<ChatTranscript {...props} pendingUserMsg="Question" />)

    expect(scrollTo).toHaveBeenCalledWith({ top: 864, behavior: "smooth" })
    expect(scrollIntoView).not.toHaveBeenCalled()
  })
})
