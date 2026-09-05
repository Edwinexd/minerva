/**
 * The two shapes a guarded turn takes in the disclosure.
 *
 * `thinking_hidden` says the extraction guard fired, not that the
 * response is empty: the server withholds the trace from the student
 * and sends it to the teacher. Rendering both off the same flag is
 * what keeps a teacher's live stream and the refetch that follows it
 * telling the same story ; they used to disagree, so the placeholder
 * flipped to the full transcript the moment generation finished.
 */
import { describe, expect, it } from "vitest"
import { render, screen } from "@testing-library/react"

import { axe } from "./a11y"
import { ThinkingBlock } from "@/components/chat/thinking-block"

const labels = {
  thinkingActive: "Thinking…",
  thinkingDoneWithDuration: "Thought for {{seconds}}s",
  thinkingDone: "Thinking",
  thinkingHidden: "Reasoning hidden by integrity guard",
  thinkingHiddenBody: "Held back under the course's academic-integrity policy.",
  thinkingHiddenRevealed: "Hidden from the student",
  thinkingHiddenRevealedBody: "The student saw a placeholder here.",
  toolCallsAriaLabel: "Tools the assistant called during research",
}

const TRACE = "First I would write the add() method, then"

describe("ThinkingBlock on a guarded turn", () => {
  it("renders the placeholder when the trace was withheld", () => {
    render(
      <ThinkingBlock
        thinkingTokens=""
        toolEvents={[]}
        active={false}
        durationMs={4200}
        hidden
        defaultOpen
        labels={labels}
      />,
    )

    expect(screen.getByText(labels.thinkingHidden)).toBeTruthy()
    expect(screen.getByText(labels.thinkingHiddenBody)).toBeTruthy()
    // The duration would otherwise leak "there was reasoning, and it
    // took this long" into the trigger.
    expect(screen.queryByText(/Thought for/)).toBeNull()
    expect(screen.queryByText(labels.thinkingHiddenRevealed)).toBeNull()
  })

  it("labels the trace instead of hiding it when the server sent one", () => {
    render(
      <ThinkingBlock
        thinkingTokens={TRACE}
        toolEvents={[{ name: "semantic_search", resultSummary: "3 chunks" }]}
        active={false}
        durationMs={4200}
        hidden
        defaultOpen
        labels={labels}
      />,
    )

    expect(screen.getByText(labels.thinkingHiddenRevealed)).toBeTruthy()
    expect(screen.getByText(labels.thinkingHiddenRevealedBody)).toBeTruthy()
    expect(screen.getByText(TRACE)).toBeTruthy()
    expect(screen.getByText("semantic_search")).toBeTruthy()
    // Normal trigger, not the student's policy-gate label.
    expect(screen.getByText("Thought for 4.2s")).toBeTruthy()
    expect(screen.queryByText(labels.thinkingHidden)).toBeNull()
  })

  it("keeps the labelled disclosure free of axe violations", async () => {
    const { container } = render(
      <ThinkingBlock
        thinkingTokens={TRACE}
        toolEvents={[{ name: "semantic_search", resultSummary: "3 chunks" }]}
        active={false}
        durationMs={4200}
        hidden
        defaultOpen
        labels={labels}
      />,
    )

    expect(await axe(container)).toHaveNoViolations()
  })

  it("leaves an unguarded turn alone", () => {
    render(
      <ThinkingBlock
        thinkingTokens={TRACE}
        toolEvents={[]}
        active={false}
        durationMs={4200}
        defaultOpen
        labels={labels}
      />,
    )

    expect(screen.getByText(TRACE)).toBeTruthy()
    expect(screen.queryByText(labels.thinkingHiddenRevealed)).toBeNull()
    expect(screen.queryByText(labels.thinkingHidden)).toBeNull()
  })
})
