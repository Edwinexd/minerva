/**
 * Shared chat-message bubble used by the regular Shibboleth chat page
 * and the LTI embed iframe.
 *
 * The two callers differ in:
 *   - i18n namespace ("student" vs "auth")
 *   - whether the assistant footer shows usage stats and feedback controls
 *
 * Both branches were copied/pasted variants of each other before this
 * was extracted; the embed copy was missing thumbs-up/down support and
 * teacher-pinned-conversation handling. Centralising here means future
 * source/feedback tweaks land in both places automatically.
 */
import React, { useCallback, useEffect, useRef, useState } from "react"
import { ChevronDown, ChevronUp } from "lucide-react"
import Markdown, { defaultUrlTransform } from "react-markdown"
import remarkGfm from "remark-gfm"

/**
 * Citation markers the writeup phase emits inline. The model is
 * prompted to use ASCII `[#N]`, but some models (notably qwen and
 * gpt-oss variants when generating in a multilingual context)
 * occasionally swap in CJK full-width brackets `【#N】` or
 * heavy-bracket variants `〔#N〕`. We accept any of those on
 * read so the citation badges still wire up; the `rewrite` pass
 * normalises them all into a single ASCII form before handing to
 * `react-markdown`.
 */
const CITATION_RE = /[[【〔]#(\d+)[\]】〕]/g

/**
 * Strip the optional `[Source: filename]` prefix off a stored
 * chunk string and return `{ source, text }`. Used both by the
 * sources panel and by `rewriteForFootnotes` when it builds
 * footnote definitions; we want the definitions to read
 * `filename ; "snippet..."` rather than the raw header.
 */
function parseChunk(
  chunk: string,
  fallback: string,
): { source: string; text: string | null } {
  const sourceMatch = chunk.match(/^\[Source: (.+?)\](\n|$)/)
  const source = sourceMatch ? sourceMatch[1] : fallback
  const hasText = sourceMatch ? chunk.length > sourceMatch[0].length : true
  const text = hasText
    ? sourceMatch
      ? chunk.slice(sourceMatch[0].length).trim()
      : chunk.trim()
    : null
  return { source, text }
}

/**
 * Sentinel used to bridge `[#N]` markers from raw writeup text
 * through `react-markdown` into a custom inline-link renderer.
 * Each marker becomes a markdown link of the form
 * `[N](minerva-cite:N)`; the `components.a` override below picks
 * those up by href scheme and replaces them with an interactive
 * citation badge that opens the sources panel and scrolls to
 * the matching row.
 */
const CITATION_HREF_PREFIX = "minerva-cite:"

/**
 * Rewrite each `[#N]` in the writeup into a markdown link that the
 * custom `<a>` renderer in `MarkdownContent` will intercept.
 * Consecutive markers `[#1][#3]` get a thin separator inserted so
 * the resulting badges don't visually collide into "13" ; same
 * fix applies in the per-source list of inbound refs.
 */
function rewriteCitationsForMarkdown(content: string): string {
  // Two-step rewrite: first insert a thin space between adjacent
  // citation markers (including across mixed bracket styles, so
  // `[#1]【#3】` still gets a separator). Then convert every
  // remaining marker into the canonical `[N](minerva-cite:N)`
  // markdown link form the custom `<a>` component intercepts.
  const padded = content.replace(
    /([[【〔]#\d+[\]】〕])(?=[[【〔]#)/g,
    "$1 ",
  )
  return padded.replace(
    CITATION_RE,
    (_m, raw) => `[${raw}](${CITATION_HREF_PREFIX}${raw})`,
  )
}

/**
 * Set of 1-based source indices the writeup actually cited.
 * Used by the bottom "N sources" panel to highlight cited chunks
 * vs ones that only fed retrieval context but never landed in the
 * answer.
 */
function extractCitedSourceIds(content: string): Set<number> {
  const ids = new Set<number>()
  for (const m of content.matchAll(CITATION_RE)) {
    const n = parseInt(m[1], 10)
    if (Number.isFinite(n)) ids.add(n)
  }
  return ids
}

export interface ChatBubbleMessage {
  id: string
  role: "user" | "assistant"
  content: string
  chunks_used: string[] | null
  tokens_prompt?: number | null
  tokens_completion?: number | null
  /**
   * When non-null, the per-message footer renders the token total
   * broken into `(A research + B writeup)`. `null` on legacy
   * single-pass messages and on user messages.
   */
  research_tokens?: number | null
  generation_ms?: number | null
  retrieval_count?: number | null
}

/**
 * Strings the bubble needs. Passed in rather than read via
 * `useTranslation` because the two call sites use different i18n
 * namespaces ("student" vs the embed-only "auth.embed").
 */
export interface ChatBubbleLabels {
  /** Text on the source toggle button, e.g. "3 sources". */
  sourceCount: (count: number) => string
  /** Fallback label when a chunk has no `[Source: ...]` prefix. */
  unknownSource: string
  /** Shown when a source has only a header and no body text. */
  sourceUnavailable: string
  /**
   * Optional usage-stats labels. When omitted the footer hides token
   * counts, generation time, "using" suffix, and the across-retrievals
   * note (which is the embed view's behaviour).
   */
  stats?: {
    tokensUsed: (count: number) => string
    /**
     * Renders the per-message research vs writeup split when the
     * message carries a `research_tokens` value. Receives the
     * pre-formatted research and writeup counts and returns the
     * complete footer fragment (e.g. ` (123 research + 456
     * writeup)`).
     */
    tokenBreakdown: (research: number, writeup: number) => string
    generationTime: (seconds: string) => string
    usingSuffix: string
    acrossRetrievals: (count: number) => string
  }
}

export function MarkdownContent({
  content,
  className,
  onCitationClick,
}: {
  content: string
  className?: string
  /**
   * When provided, `[#N]` markers in `content` are intercepted and
   * rendered as interactive citation badges that call this with
   * the 1-based source index. Otherwise badges render as plain
   * superscript text with no click behaviour.
   */
  onCitationClick?: (sourceIndex: number) => void
}) {
  const rewritten = onCitationClick ? rewriteCitationsForMarkdown(content) : content
  return (
    <div className={`prose prose-sm dark:prose-invert max-w-none ${className || ""}`}>
      <Markdown
        remarkPlugins={[remarkGfm]}
        // react-markdown sanitises link URLs by default and only
        // accepts http(s)/mailto/tel/relative schemes ; anything
        // else (including our `minerva-cite:N` scheme used for
        // citation badges) gets stripped to an empty href, which
        // makes the rendered `<a>` reload the page on click.
        // Override to let `minerva-cite:` through verbatim while
        // preserving the default sanitisation for everything else.
        urlTransform={(url) =>
          typeof url === "string" && url.startsWith(CITATION_HREF_PREFIX)
            ? url
            : defaultUrlTransform(url)
        }
        components={{
          a: ({ href, children, ...rest }) => {
            if (
              onCitationClick &&
              typeof href === "string" &&
              href.startsWith(CITATION_HREF_PREFIX)
            ) {
              const n = parseInt(href.slice(CITATION_HREF_PREFIX.length), 10)
              if (Number.isFinite(n)) {
                return (
                  <button
                    type="button"
                    onClick={(e) => {
                      e.preventDefault()
                      onCitationClick(n)
                    }}
                    className="inline-flex items-center justify-center min-w-[1.1rem] h-4 px-1 mx-0.5 rounded text-[0.65rem] font-semibold tabular-nums align-super bg-primary/15 text-primary hover:bg-primary/25 no-underline cursor-pointer"
                    aria-label={`Source ${n}`}
                  >
                    {n}
                  </button>
                )
              }
            }
            return (
              <a href={href} {...rest}>
                {children}
              </a>
            )
          },
        }}
      >
        {rewritten}
      </Markdown>
    </div>
  )
}

export function ChatBubble({
  message,
  labels,
  feedbackSlot = null,
}: {
  message: ChatBubbleMessage
  labels: ChatBubbleLabels
  /** Rendered inside the assistant footer (e.g. <FeedbackControls/>). */
  feedbackSlot?: React.ReactNode
}) {
  const isUser = message.role === "user"
  const [showSources, setShowSources] = useState(false)
  const chunks = message.chunks_used
  const citedSourceIds = !isUser
    ? extractCitedSourceIds(message.content)
    : new Set<number>()
  const hasCitations = citedSourceIds.size > 0
  // Show-uncited is derived from the message + a per-mount user
  // override. `useState(initial)` would capture the initial
  // computed value once and never re-derive when the underlying
  // `hasCitations` changes (e.g. content streams in / refetches
  // with citations after first render); the captured-once stale
  // value was the "broken until page refresh" bug. Tracking a
  // nullable override + deriving the effective value on every
  // render fixes it without forcing a useEffect+setState.
  const [userShowUncitedOverride, setUserShowUncitedOverride] = useState<
    boolean | null
  >(null)
  const showUncited = userShowUncitedOverride ?? !hasCitations
  // The trigger button's count reflects what the panel will show:
  // cited rows when citations exist, otherwise everything. Keeps
  // the "N sources" label honest about what clicking actually
  // reveals.
  const visibleSourceCount = hasCitations
    ? citedSourceIds.size
    : (chunks?.length ?? 0)

  // Per-source row refs so a citation badge click can scroll the
  // matching row into view inside the sources panel.
  const sourceRefs = useRef<Map<number, HTMLDivElement | null>>(new Map())
  // Briefly-highlighted source row (1-based id) after a badge
  // click; the effect below scrolls it into view and fades it.
  const [focusedSource, setFocusedSource] = useState<number | null>(null)

  useEffect(() => {
    if (focusedSource == null) return
    // Defer one tick so the lazily-mounted sources panel has a
    // chance to render the row we're scrolling to.
    const scrollTimer = setTimeout(() => {
      sourceRefs.current.get(focusedSource)?.scrollIntoView({
        behavior: "smooth",
        block: "center",
      })
    }, 0)
    const fadeTimer = setTimeout(() => setFocusedSource(null), 1500)
    return () => {
      clearTimeout(scrollTimer)
      clearTimeout(fadeTimer)
    }
  }, [focusedSource])

  const handleCitationClick = useCallback(
    (n: number) => {
      // Open the panel; reveal uncited rows too in case the model
      // cited a chunk that the cited-set didn't dedupe. Then push
      // the focus state through a brief null transition so the
      // same badge clicked twice still re-triggers scroll/fade.
      setShowSources(true)
      // A badge click may target a row that's currently filtered
      // out (uncited or out-of-range). Force the panel to show
      // everything so the scroll target exists.
      setUserShowUncitedOverride(true)
      setFocusedSource(null)
      setTimeout(() => setFocusedSource(n), 0)
    },
    [],
  )

  return (
    <div className={`flex ${isUser ? "justify-end" : "justify-start"}`}>
      <div
        className={`rounded-lg px-4 py-2 max-w-[80%] ${
          isUser ? "bg-primary text-primary-foreground" : "bg-muted"
        }`}
      >
        {isUser ? (
          <p className="text-sm whitespace-pre-wrap">{message.content}</p>
        ) : (
          <MarkdownContent
            content={message.content}
            onCitationClick={handleCitationClick}
          />
        )}
        {!isUser && (
          <div className="flex items-center gap-1 mt-2 text-xs text-muted-foreground flex-wrap">
            {labels.stats && message.tokens_prompt != null && (
              <span>
                {labels.stats.tokensUsed(
                  message.tokens_prompt + (message.tokens_completion ?? 0),
                )}
                {/*
                  When the message has a stored research-token
                  subtotal, append ` (A research + B writeup)`
                  immediately after the headline `N tokens`
                  number. NULL on legacy single-pass messages, in
                  which case this whole fragment is skipped and
                  the headline stays opaque.
                */}
                {message.research_tokens != null &&
                  message.tokens_prompt != null &&
                  labels.stats.tokenBreakdown(
                    message.research_tokens,
                    message.tokens_prompt +
                      (message.tokens_completion ?? 0) -
                      message.research_tokens,
                  )}
                {message.generation_ms != null &&
                  labels.stats.generationTime(
                    (message.generation_ms / 1000).toFixed(1),
                  )}
                {chunks && chunks.length > 0 && labels.stats.usingSuffix}
              </span>
            )}
            {chunks && chunks.length > 0 && visibleSourceCount > 0 && (
              <>
                <button
                  className="underline hover:text-foreground"
                  onClick={() => setShowSources(!showSources)}
                >
                  {labels.sourceCount(visibleSourceCount)}
                  {showSources ? (
                    <ChevronUp className="inline w-3 h-3 ml-0.5" />
                  ) : (
                    <ChevronDown className="inline w-3 h-3 ml-0.5" />
                  )}
                </button>
                {labels.stats &&
                  message.retrieval_count != null &&
                  message.retrieval_count > 1 && (
                    <span>{labels.stats.acrossRetrievals(message.retrieval_count)}</span>
                  )}
              </>
            )}
            {feedbackSlot}
          </div>
        )}
        {showSources && chunks && (
          <div className="mt-2 space-y-2 border-t pt-2">
            {chunks
              .map((chunk, i) => ({ chunk, sourceId: i + 1 }))
              // Default to cited-only ; the model's `[#N]` markers
              // pick the rows that mattered. The "Show N more"
              // toggle below reveals the retrieval-only chunks for
              // users who want the full retrieval context.
              .filter(({ sourceId }) =>
                showUncited ? true : citedSourceIds.has(sourceId),
              )
              .map(({ chunk, sourceId }) => {
                const { source, text } = parseChunk(chunk, labels.unknownSource)
                const wasCited = citedSourceIds.has(sourceId)
                const isFocused = focusedSource === sourceId
                return (
                  <div
                    key={sourceId}
                    ref={(el) => {
                      sourceRefs.current.set(sourceId, el)
                    }}
                    className={`text-xs rounded p-1.5 -m-1.5 transition-colors duration-300 ${
                      isFocused ? "bg-primary/10 ring-1 ring-primary/40" : ""
                    }`}
                  >
                    <span className="font-medium text-muted-foreground inline-flex items-center gap-1.5">
                      <span
                        className={`inline-flex items-center justify-center min-w-[1.25rem] h-4 px-1 rounded text-[0.65rem] font-semibold tabular-nums ${
                          wasCited
                            ? "bg-primary/15 text-primary"
                            : "bg-muted-foreground/15 text-muted-foreground/70"
                        }`}
                        aria-label={
                          wasCited
                            ? `Source ${sourceId}, cited in the reply`
                            : `Source ${sourceId}, not directly cited`
                        }
                      >
                        {sourceId}
                      </span>
                      <span>{source}</span>
                    </span>
                    {text ? (
                      <p className="text-muted-foreground/80 mt-0.5 line-clamp-3">{text}</p>
                    ) : (
                      <p className="text-muted-foreground/60 mt-0.5 italic">
                        {labels.sourceUnavailable}
                      </p>
                    )}
                  </div>
                )
              })}
            {/* Tail toggle for the retrieval-only chunks. Hidden
                when there are none (all retrieved chunks were
                cited). */}
            {(() => {
              const uncitedCount = chunks.filter(
                (_c, i) => !citedSourceIds.has(i + 1),
              ).length
              if (uncitedCount === 0) return null
              return (
                <button
                  type="button"
                  className="text-[11px] underline text-muted-foreground hover:text-foreground"
                  onClick={() => setUserShowUncitedOverride(!showUncited)}
                >
                  {showUncited
                    ? `Hide ${uncitedCount} retrieved but not cited`
                    : `Show ${uncitedCount} retrieved but not cited`}
                </button>
              )
            })()}
          </div>
        )}
      </div>
    </div>
  )
}
