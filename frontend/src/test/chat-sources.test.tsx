/**
 * The sources panel's excerpt rows.
 *
 * A retrieval chunk is up to the chunker's full `chunk_size`, and the
 * panel clamps it to three lines; the expand toggle is the only way
 * to read the rest. jsdom has no layout, so the overflow measurement
 * that decides whether the toggle appears is driven here by stubbing
 * the two layout getters the component reads.
 */
import { describe, expect, it } from "vitest"
import { fireEvent, screen } from "@testing-library/react"

import { axe, renderWithProviders } from "./a11y"
import {
  ChatBubble,
  type ChatBubbleLabels,
  type ChatBubbleMessage,
} from "@/components/chat/chat-bubble"

// Only the per-namespace wording is a prop; the panel's own controls
// come from `common`, which is why this renders with the i18n provider.
const labels: ChatBubbleLabels = {
  sourceCount: (count) => `${count} sources`,
  unknownSource: "Unknown source",
  sourceUnavailable: "Source unavailable",
}

const CHUNK_BODY = Array.from(
  { length: 12 },
  (_i, n) => `Line ${n + 1} of the retrieved chunk.`,
).join("\n")

const message: ChatBubbleMessage = {
  id: "m1",
  role: "assistant",
  content: "Merge sort splits the list first [#1].",
  chunks_used: [`[Source: lecture-04.pdf]\n${CHUNK_BODY}`],
}

/**
 * Make every element report itself as clamped (or not), the way a
 * real browser would for a 12-line chunk in a 3-line box.
 */
function stubOverflow(overflowing: boolean) {
  for (const [prop, value] of [
    ["scrollHeight", overflowing ? 240 : 40],
    ["clientHeight", 40],
  ] as const) {
    Object.defineProperty(HTMLElement.prototype, prop, {
      configurable: true,
      get: () => value,
    })
  }
}

function openSources() {
  renderWithProviders(<ChatBubble message={message} labels={labels} />)
  fireEvent.click(screen.getByRole("button", { name: /1 sources/ }))
}

describe("source excerpts", () => {
  it("expands a clamped chunk in place and collapses it again", () => {
    stubOverflow(true)
    openSources()

    const toggle = screen.getByRole("button", { name: "Show full excerpt" })
    expect(toggle.getAttribute("aria-expanded")).toBe("false")
    const excerpt = document.getElementById(toggle.getAttribute("aria-controls")!)
    expect(excerpt?.className).toContain("line-clamp-3")
    // The whole chunk is in the DOM either way; the clamp is what
    // decides how much of it the reader gets to see.
    expect(excerpt?.textContent).toContain("Line 12 of the retrieved chunk.")

    fireEvent.click(toggle)

    const collapse = screen.getByRole("button", { name: "Show less" })
    expect(collapse.getAttribute("aria-expanded")).toBe("true")
    expect(excerpt?.className).not.toContain("line-clamp-3")

    fireEvent.click(collapse)
    expect(
      screen.getByRole("button", { name: "Show full excerpt" }),
    ).toBeTruthy()
  })

  it("offers no toggle when the chunk already fits", () => {
    stubOverflow(false)
    openSources()

    expect(screen.queryByRole("button", { name: "Show full excerpt" })).toBeNull()
  })

  it("has no accessibility violations with an expanded excerpt", async () => {
    stubOverflow(true)
    openSources()
    fireEvent.click(screen.getByRole("button", { name: "Show full excerpt" }))

    expect(await axe(document.body)).toHaveNoViolations()
  })
})
