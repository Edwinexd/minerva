/**
 * A block of document text that is clamped to a few lines with a
 * toggle for the rest of it. Used wherever the UI shows a retrieval
 * chunk: the chat sources panel and the teacher's RAG search results.
 * Both used to clamp with no way past the clamp, so the tail of every
 * chunk was unreachable.
 *
 * The toggle is measured rather than assumed: a short chunk never
 * clamps at the container's width, and offering "show more" on text
 * that is already whole is worse than not offering it. Once expanded
 * the clamp is gone, so the element stops overflowing ; the
 * measurement is skipped in that state to keep the collapse toggle
 * mounted. jsdom reports every element as zero-sized, so `overflows`
 * stays false there unless a test stubs the layout getters.
 *
 * Chunk text is extracted document text, not prose written for the
 * browser: `whitespace-pre-wrap` keeps the line breaks that carry code
 * and list structure, and `break-words` stops a long URL from widening
 * the container.
 *
 * Strings come from `common` rather than from props. The two call
 * sites sit in different i18n namespaces ("student" / "auth.embed" for
 * the bubble, "teacher" for the RAG page), and threading identical
 * labels through each of them would mean a copy of the same two
 * strings in every namespace.
 */
import { useLayoutEffect, useId, useRef, useState } from "react"
import { useTranslation } from "react-i18next"

/**
 * Tailwind scans for complete class names, so the clamp classes have
 * to appear literally rather than as `line-clamp-${lines}`.
 */
const CLAMP_CLASSES = {
  2: "line-clamp-2",
  3: "line-clamp-3",
  4: "line-clamp-4",
} as const

export function ClampedText({
  text,
  lines = 3,
  className,
}: {
  text: string
  /** Lines to show while collapsed. */
  lines?: keyof typeof CLAMP_CLASSES
  /** Typography for the text itself; the toggle keeps its own. */
  className?: string
}) {
  const { t } = useTranslation("common")
  const [expanded, setExpanded] = useState(false)
  const [overflows, setOverflows] = useState(false)
  const textRef = useRef<HTMLParagraphElement | null>(null)
  const textId = useId()

  useLayoutEffect(() => {
    const el = textRef.current
    if (!el || expanded) return
    const measure = () => setOverflows(el.scrollHeight > el.clientHeight + 1)
    measure()
    window.addEventListener("resize", measure)
    return () => window.removeEventListener("resize", measure)
  }, [text, expanded])

  return (
    <>
      <p
        id={textId}
        ref={textRef}
        className={`whitespace-pre-wrap break-words ${
          expanded ? "" : CLAMP_CLASSES[lines]
        } ${className || ""}`}
      >
        {text}
      </p>
      {overflows && (
        <button
          type="button"
          className="mt-0.5 text-[11px] underline text-muted-foreground hover:text-foreground"
          aria-expanded={expanded}
          aria-controls={textId}
          onClick={() => setExpanded(!expanded)}
        >
          {expanded ? t("clampedText.showLess") : t("clampedText.showFull")}
        </button>
      )}
    </>
  )
}
