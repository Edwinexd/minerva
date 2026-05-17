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
  /** Disclosure trigger text after `thinking_done` fires. */
  thinkingDone: string
  /** aria-label for the toolbar listing tool calls. */
  toolCallsAriaLabel: string
}

export interface ThinkingBlockProps {
  thinkingTokens: string
  toolEvents: ToolEvent[]
  active: boolean
  labels: ThinkingBlockLabels
}

export function ThinkingBlock({
  thinkingTokens,
  toolEvents,
  active,
  labels,
}: ThinkingBlockProps) {
  // Auto-expand while research is streaming, auto-collapse once
  // it's done. The user can override that with a manual toggle;
  // null = "follow `active`", true/false = explicit user choice.
  // Derived (no useEffect ; the linter forbids cascading
  // setState in effects).
  const [userOpen, setUserOpen] = useState<boolean | null>(null)
  const open = userOpen ?? active

  const trigger = active ? labels.thinkingActive : labels.thinkingDone

  return (
    <details
      open={open}
      onToggle={(e) => {
        const nextOpen = (e.currentTarget as HTMLDetailsElement).open
        setUserOpen(nextOpen)
      }}
      className="text-xs rounded-md border border-muted-foreground/15 bg-background/40"
    >
      <summary className="cursor-pointer select-none px-3 py-1.5 font-medium text-muted-foreground hover:text-foreground">
        <span className="inline-flex items-center gap-2">
          {active && (
            <span
              aria-hidden="true"
              className="inline-block h-1.5 w-1.5 animate-pulse rounded-full bg-muted-foreground"
            />
          )}
          {trigger}
          {toolEvents.length > 0 && (
            <span className="text-muted-foreground/70">
              {" "}
              ({toolEvents.length})
            </span>
          )}
        </span>
      </summary>
      <div className="space-y-2 border-t border-muted-foreground/10 px-3 py-2">
        {toolEvents.length > 0 && (
          <ul
            aria-label={labels.toolCallsAriaLabel}
            className="space-y-1 text-muted-foreground"
          >
            {toolEvents.map((ev, i) => (
              <li key={i} className="flex flex-wrap items-baseline gap-1">
                <code className="font-mono text-foreground/80">{ev.name}</code>
                {ev.args !== undefined && (
                  <code className="font-mono text-muted-foreground/80 truncate max-w-[40ch]">
                    ({truncateArgs(ev.args)})
                  </code>
                )}
                {ev.resultSummary && (
                  <span className="text-muted-foreground/80">
                    {"→"} {ev.resultSummary}
                  </span>
                )}
              </li>
            ))}
          </ul>
        )}
        {thinkingTokens && (
          <div className="prose prose-xs max-w-none text-muted-foreground">
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
