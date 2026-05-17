/**
 * Collapsible "Thinking" disclosure rendered above an in-progress
 * assistant reply when the course has `tool_use_enabled = TRUE`.
 *
 * Surfaces the research-phase stream the backend emits:
 *   - `thinking_token` ; concatenated into `thinkingTokens` and
 *     rendered as markdown so the model's thoughts read like a
 *     short scratchpad.
 *   - `tool_call` + `tool_result` pairs ; rendered as a compact
 *     bulleted log so the student can see which course materials
 *     the bot looked up.
 *
 * Defaults to expanded while `active === true` (research phase
 * still streaming) and collapses once `thinking_done` fires. The
 * user can also toggle manually.
 */
import { useState } from "react"

import { MarkdownContent } from "./chat-bubble"
import type { ToolEvent } from "./use-chat-stream"

export interface ThinkingBlockLabels {
  /** Disclosure trigger text while research is still streaming. */
  thinkingActive: string
  /**
   * Trigger text after `thinking_done` fires when we know the
   * duration. The frontend interpolates `{{seconds}}` (rounded to
   * one decimal) into the string, e.g. "Thought for 3.2s".
   */
  thinkingDoneWithDuration: string
  /**
   * Fallback trigger text when we have a finished disclosure but
   * no duration available (e.g. legacy rows that pre-date the
   * `thinking_ms` column).
   */
  thinkingDone: string
  /** aria-label for the toolbar listing tool calls. */
  toolCallsAriaLabel: string
}

export interface ThinkingBlockProps {
  thinkingTokens: string
  toolEvents: ToolEvent[]
  active: boolean
  /**
   * Wall-clock duration of the research phase, in milliseconds.
   * `null` while the phase is still streaming and on historical
   * messages without a stored duration.
   */
  durationMs: number | null
  labels: ThinkingBlockLabels
}

export function ThinkingBlock({
  thinkingTokens,
  toolEvents,
  active,
  durationMs,
  labels,
}: ThinkingBlockProps) {
  // Auto-expand while research is streaming, auto-collapse once
  // it's done. The user can override that with a manual toggle;
  // null = "follow `active`", true/false = explicit user choice.
  // Derived (no useEffect ; the linter forbids cascading
  // setState in effects).
  const [userOpen, setUserOpen] = useState<boolean | null>(null)
  const open = userOpen ?? active

  // Trigger text: while streaming -> "Thinking...", once done and
  // we have a duration -> "Thought for {{seconds}}s", otherwise
  // fall back to "Thinking" (legacy messages without `thinking_ms`).
  let trigger: string
  if (active) {
    trigger = labels.thinkingActive
  } else if (durationMs !== null) {
    const seconds = (durationMs / 1000).toFixed(1)
    trigger = labels.thinkingDoneWithDuration.replace("{{seconds}}", seconds)
  } else {
    trigger = labels.thinkingDone
  }

  return (
    <details
      open={open}
      onToggle={(e) => {
        const nextOpen = (e.currentTarget as HTMLDetailsElement).open
        setUserOpen(nextOpen)
      }}
      // Subtle background card so the disclosure reads as its own
      // section (metadata about the message below) but at a lower
      // visual weight than the answer bubble itself. Contrast on
      // the trigger text is full `text-muted-foreground` (not the
      // washed-out /80) so it stays legible against the muted bg.
      className="text-xs text-muted-foreground bg-muted/40 border border-muted-foreground/15 rounded-md px-2.5 py-1.5"
    >
      <summary className="cursor-pointer select-none hover:text-foreground inline-flex items-center gap-1.5 list-none [&::-webkit-details-marker]:hidden">
        {/*
          Chevron arrow: SVG `>` shape, pointing RIGHT when collapsed
          and rotating 90deg to point DOWN (`v`) when open. Standard
          disclosure pattern. Tailwind's `[details[open]_&]:rotate-90`
          flips it on the parent <details>'s open attribute, no JS
          state needed.
        */}
        <svg
          aria-hidden="true"
          viewBox="0 0 6 10"
          className="inline-block h-2.5 w-2 transition-transform [details[open]_&]:rotate-90"
        >
          <path
            d="M1 1 L5 5 L1 9"
            stroke="currentColor"
            strokeWidth="1.5"
            strokeLinecap="round"
            strokeLinejoin="round"
            fill="none"
          />
        </svg>
        {active && (
          <span
            aria-hidden="true"
            className="inline-block h-1 w-1 animate-pulse rounded-full bg-muted-foreground"
          />
        )}
        <span>{trigger}</span>
        {toolEvents.length > 0 && (
          <span className="text-muted-foreground/60">
            ({toolEvents.length})
          </span>
        )}
      </summary>
      <div className="mt-1.5 space-y-1.5">
        {toolEvents.length > 0 && (
          <ul
            aria-label={labels.toolCallsAriaLabel}
            className="space-y-1"
          >
            {toolEvents.map((ev, i) => (
              <li key={i}>
                {/*
                  Per-call disclosure: the line is its own collapsible
                  <details>, so a user can click any tool call to see
                  the raw JSON result the dispatcher returned to the
                  model. Renders only when `result` is present; on
                  legacy rows without a stored result we degrade to a
                  plain row.
                */}
                {ev.result !== undefined ? (
                  <details className="group">
                    <summary className="cursor-pointer select-none list-none flex flex-wrap items-baseline gap-1 hover:text-foreground [&::-webkit-details-marker]:hidden">
                      <svg
                        aria-hidden="true"
                        viewBox="0 0 6 10"
                        className="inline-block h-2.5 w-2 transition-transform group-open:rotate-90 shrink-0 self-center"
                      >
                        <path
                          d="M1 1 L5 5 L1 9"
                          stroke="currentColor"
                          strokeWidth="1.5"
                          strokeLinecap="round"
                          strokeLinejoin="round"
                          fill="none"
                        />
                      </svg>
                      <code className="font-mono text-foreground">{ev.name}</code>
                      {ev.args !== undefined && (
                        <code className="font-mono text-muted-foreground truncate max-w-[40ch]">
                          ({truncateArgs(ev.args)})
                        </code>
                      )}
                      {ev.resultSummary && (
                        <span className="text-muted-foreground">
                          {"→"} {ev.resultSummary}
                        </span>
                      )}
                    </summary>
                    <pre className="mt-1 ml-3 max-h-64 overflow-auto rounded border border-muted-foreground/15 bg-background/60 p-2 text-[11px] leading-snug text-foreground/90 whitespace-pre-wrap break-words">
                      {prettyPrintResult(ev.result)}
                    </pre>
                  </details>
                ) : (
                  <div className="flex flex-wrap items-baseline gap-1 pl-3">
                    <code className="font-mono text-foreground">{ev.name}</code>
                    {ev.args !== undefined && (
                      <code className="font-mono text-muted-foreground truncate max-w-[40ch]">
                        ({truncateArgs(ev.args)})
                      </code>
                    )}
                    {ev.resultSummary && (
                      <span className="text-muted-foreground">
                        {"→"} {ev.resultSummary}
                      </span>
                    )}
                  </div>
                )}
              </li>
            ))}
          </ul>
        )}
        {thinkingTokens && (
          // Expanded thinking content reads at the regular message
          // body font size (no `text-xs`, no italics). Use the full
          // foreground colour for contrast against the muted card
          // background; the disclosure TRIGGER stays muted because
          // it's metadata, but the contents are the model's actual
          // reasoning and need to be legible.
          <div className="prose prose-sm max-w-none text-foreground">
            <MarkdownContent content={thinkingTokens} />
          </div>
        )}
      </div>
    </details>
  )
}

function truncateArgs(args: unknown): string {
  try {
    const s = typeof args === "string" ? args : JSON.stringify(args)
    if (s.length <= 80) return s
    return s.slice(0, 77) + "..."
  } catch {
    return "?"
  }
}

/**
 * Render a tool-call result for the per-call expanded view.
 * Pretty-prints JSON for readability; strings pass through.
 */
function prettyPrintResult(result: unknown): string {
  if (typeof result === "string") return result
  try {
    return JSON.stringify(result, null, 2)
  } catch {
    return String(result)
  }
}
