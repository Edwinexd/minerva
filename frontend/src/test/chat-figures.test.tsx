/**
 * Figures shown under an assistant reply: images from the course's slides
 * and PDFs, each a link to the full-size image with alt text from its
 * caption (or its origin when OCR found no caption).
 */
import { describe, expect, it } from "vitest"
import { screen } from "@testing-library/react"

import { axe, renderWithProviders } from "./a11y"
import {
  ChatBubble,
  type ChatBubbleLabels,
  type ChatBubbleMessage,
} from "@/components/chat/chat-bubble"

const labels: ChatBubbleLabels = {
  sourceCount: (count) => `${count} sources`,
  unknownSource: "Unknown source",
  sourceUnavailable: "Source unavailable",
}

const message: ChatBubbleMessage = {
  id: "m1",
  role: "assistant",
  content: "The transformer stacks attention layers.",
  chunks_used: null,
  figures_used: [
    {
      id: "f1",
      origin: "Introduction to Generative AI, slide 7",
      caption: "The 2017 architecture",
      image_url: "/api/embed/figures/signed-1",
    },
    {
      id: "f2",
      origin: "teacher-guide.pdf, page 5",
      caption: null,
      image_url: "/api/embed/figures/signed-2",
    },
  ],
}

describe("reply figures", () => {
  it("shows each figure as a captioned link to the full image", async () => {
    const { container } = renderWithProviders(<ChatBubble message={message} labels={labels} />)

    const captioned = screen.getByRole("img", { name: "The 2017 architecture" })
    expect(captioned.getAttribute("src")).toBe("/api/embed/figures/signed-1")
    // No OCR caption: the alt text falls back to where the figure is from.
    screen.getByRole("img", { name: "Figure from teacher-guide.pdf, page 5" })

    const link = screen.getByRole("link", { name: "Open Introduction to Generative AI, slide 7 full size" })
    expect(link.getAttribute("href")).toBe("/api/embed/figures/signed-1")
    expect(link.getAttribute("target")).toBe("_blank")
    screen.getByText("teacher-guide.pdf, page 5")

    expect(await axe(container)).toHaveNoViolations()
  })

  it("places figures where the reply's markers ask for them", async () => {
    const placed: ChatBubbleMessage = {
      ...message,
      content: [
        "Minerva runs as a Moodle activity.",
        "",
        "[Figure 2]",
        "",
        "The model behind it is a transformer, shown here:",
        "[Figur 1]",
        "A marker for a figure that is not there renders nothing: [Figure 9]",
      ].join("\n"),
    }
    const { container } = renderWithProviders(<ChatBubble message={placed} labels={labels} />)

    // Both placed inline, in reply order, inside the reply's prose.
    const prose = container.querySelector(".prose")!
    const inline = Array.from(prose.querySelectorAll("figure img")).map((img) => img.getAttribute("alt"))
    expect(inline).toEqual(["Figure from teacher-guide.pdf, page 5", "The 2017 architecture"])
    // Inline figures carry their caption in the figcaption.
    screen.getByText("The 2017 architecture (Introduction to Generative AI, slide 7)")
    // Nothing left unplaced, so no strip under the reply; no stray marker text.
    expect(screen.queryByRole("region", { name: "Figures from the course material" })).toBeNull()
    expect(container.textContent).not.toContain("[Figure")
    expect(container.textContent).not.toContain("[Figur")
    // A figure never ends up inside a paragraph.
    expect(prose.querySelector("p figure")).toBeNull()

    expect(await axe(container)).toHaveNoViolations()
  })

  it("keeps unplaced figures in the strip under the reply", () => {
    renderWithProviders(
      <ChatBubble message={{ ...message, content: "See this:\n\n[Figure 1]" }} labels={labels} />,
    )
    const strip = screen.getByRole("region", { name: "Figures from the course material" })
    const stripAlts = Array.from(strip.querySelectorAll("img")).map((img) => img.getAttribute("alt"))
    expect(stripAlts).toEqual(["Figure from teacher-guide.pdf, page 5"])
  })

  it("renders nothing extra without figures", () => {
    renderWithProviders(<ChatBubble message={{ ...message, figures_used: null }} labels={labels} />)
    expect(screen.queryByRole("img")).toBeNull()
  })
})
