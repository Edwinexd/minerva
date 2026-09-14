import { Button } from "@/components/ui/button"
import { X } from "lucide-react"
import type {
  ConversationLimitAction,
  ConversationLimitState,
} from "./conversation-limit-state"

export interface ConversationLimitLabels {
  topicTitle: string
  topicBody: string
  nudgeTitle: string
  nudgeBody: string
  /** Nudge body when the long thread has also switched topic, so the
   * action carries the new question rather than a recap. */
  nudgeTopicBody: string
  blockedTitle: string
  blockedBody: string
  /** Blocked body for the same combined case as `nudgeTopicBody`. */
  blockedTopicBody: string
  continueAction: string
  continueWorking: string
  /** Action label when the button branches (carries the new-topic
   * exchange) rather than splitting with a recap. */
  newChatAction: string
  dismiss: string
}

/**
 * Banner above the composer asking the student to carry on in a fresh
 * conversation, either because this one is getting long or because they
 * have moved on to a new question.
 *
 * Rendered by `ChatSurface` for both the Shibboleth and embed surfaces.
 * The `blocked` variant is deliberately not dismissible: the composer is
 * hidden in that state, so dismissing would leave a chat window with no
 * visible explanation for why nothing can be typed.
 */
export function ConversationLimitNotice({
  state,
  action,
  labels,
  onAction,
  continuing,
  onDismiss,
  error,
}: {
  state: Exclude<ConversationLimitState, "ok">
  /** Which endpoint `onAction` calls; picks the body copy and label. */
  action: ConversationLimitAction
  labels: ConversationLimitLabels
  onAction: () => void
  continuing: boolean
  onDismiss?: () => void
  error?: string | null
}) {
  const blocked = state === "blocked"
  const branching = action === "branch"
  const title =
    blocked
      ? labels.blockedTitle
      : state === "topic"
        ? labels.topicTitle
        : labels.nudgeTitle
  const body =
    blocked
      ? branching ? labels.blockedTopicBody : labels.blockedBody
      : state === "topic"
        ? labels.topicBody
        : branching ? labels.nudgeTopicBody : labels.nudgeBody
  return (
    <div
      // `alert` for the block (the student is stopped and needs to know
      // now); `status` for the nudge, which is advisory and must not
      // interrupt a screen-reader user mid-compose.
      role={blocked ? "alert" : "status"}
      className={`flex items-start gap-3 rounded-md border px-3 py-2 text-sm ${
        blocked
          ? "border-amber-400 bg-amber-50 text-amber-900 dark:border-amber-600 dark:bg-amber-950/40 dark:text-amber-100"
          : "border-border bg-muted/40 text-muted-foreground"
      }`}
    >
      <div className="flex-1 min-w-0 space-y-1">
        <p className="font-medium">{title}</p>
        <p>{body}</p>
        {error && <p className="text-destructive">{error}</p>}
      </div>
      <div className="flex items-center gap-1 shrink-0">
        <Button
          size="sm"
          variant={blocked ? "default" : "outline"}
          onClick={onAction}
          disabled={continuing}
        >
          {continuing
            ? labels.continueWorking
            : branching
              ? labels.newChatAction
              : labels.continueAction}
        </Button>
        {!blocked && onDismiss && (
          <Button
            size="sm"
            variant="ghost"
            onClick={onDismiss}
            aria-label={labels.dismiss}
          >
            <X className="w-4 h-4" />
          </Button>
        )}
      </div>
    </div>
  )
}

/**
 * The "picked up from an earlier chat" line shown at the top of a
 * continuation's transcript.
 *
 * The recap is otherwise invisible state: it is injected into the system
 * prompt, so without this the assistant would appear to know things the
 * student never said in this conversation.
 */
export function CarryoverNote({
  summary,
  label,
}: {
  summary: string
  label: string
}) {
  return (
    <details className="rounded-md border bg-muted/30 px-3 py-2 text-sm text-muted-foreground">
      <summary className="cursor-pointer font-medium">{label}</summary>
      <p className="mt-2 whitespace-pre-wrap">{summary}</p>
    </details>
  )
}
