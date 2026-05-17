import { RelativeTime } from "@/components/relative-time"
import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query"
import { useTranslation } from "react-i18next"
import { allConversationsQuery, conversationDetailQuery, conversationFlagKindsQuery, courseFeedbackStatsQuery, popularTopicsQuery } from "@/lib/queries"
import { api } from "@/lib/api"
import { Button } from "@/components/ui/button"
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card"
import { Label } from "@/components/ui/label"
import { Textarea } from "@/components/ui/textarea"
import { Badge } from "@/components/ui/badge"
import { Separator } from "@/components/ui/separator"
import { Skeleton } from "@/components/ui/skeleton"
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select"
import Markdown from "react-markdown"
import remarkGfm from "remark-gfm"
import React, { useMemo, useState } from "react"
import type { ConversationFlag, ConversationWithUser, MessageFeedback, TeacherNote } from "@/lib/types"
import { FEEDBACK_CATEGORIES } from "@/lib/types"

function useCategoryLabel() {
  const { t } = useTranslation("teacher")
  return (value: string | null): string => {
    if (!value) return t("conversations.otherCategory")
    return FEEDBACK_CATEGORIES.find((c) => c.value === value)?.label ?? value
  }
}

export function ConversationsPage({ useParams }: { useParams: () => { courseId: string } }) {
  const { courseId } = useParams()
  const { t } = useTranslation("teacher")
  const categoryLabel = useCategoryLabel()
  const { data: conversations, isLoading } = useQuery(allConversationsQuery(courseId))
  const { data: topics, isLoading: topicsLoading } = useQuery(popularTopicsQuery(courseId))
  const { data: feedbackStats } = useQuery(courseFeedbackStatsQuery(courseId))
  // Flag-kind map keyed by conversation id. Drives the per-row
  // "extraction guard tripped" badge in the conversation list. Same
  // shape as the backend `flag_kinds_by_conversation` returns.
  const { data: flagKinds } = useQuery(conversationFlagKindsQuery(courseId))
  const [expandedId, setExpandedId] = useState<string | null>(null)
  const [selectedTopic, setSelectedTopic] = useState<string | null>(null)
  const [activeTab, setActiveTab] = useState<"all" | "flagged" | "unreviewed">("all")
  const queryClient = useQueryClient()

  // Mark a conversation as reviewed by the teaching team when the
  // teacher expands the row. Per the product call "read ==
  // reviewed"; opening the conversation IS the review. Course-shared
  // (any teacher / TA / owner / admin's view counts) so the
  // "Unreviewed" tab and per-row dot clear for the whole team.
  // Extraction-guard flags + unaddressed downvotes are NOT auto-
  // cleared by viewing; they need explicit Acknowledge clicks.
  const markReviewedMutation = useMutation({
    mutationFn: (cid: string) =>
      api.post(`/courses/${courseId}/conversations/${cid}/mark-read`, {}),
    onSuccess: () => {
      queryClient.invalidateQueries({
        queryKey: ["courses", courseId, "conversations"],
      })
    },
  })

  const pinMutation = useMutation({
    mutationFn: ({ cid, pinned }: { cid: string; pinned: boolean }) =>
      api.put(`/courses/${courseId}/conversations/${cid}/pin`, { pinned }),
    onSuccess: () => {
      queryClient.invalidateQueries({
        queryKey: ["courses", courseId, "conversations"],
      })
    },
  })

  const activeTopic = useMemo(
    () => topics?.find((t) => t.topic === selectedTopic) ?? null,
    [topics, selectedTopic],
  )

  const topicConvIds = useMemo(
    () => activeTopic ? new Set(activeTopic.conversation_ids) : null,
    [activeTopic],
  )

  // A conversation lands in the "Flagged" tab when either
  // signal is true:
  //   1. Unaddressed student downvotes (existing behaviour).
  //   2. The extraction guard fired in a way that warrants a
  //      teacher's attention; i.e. the constraint activated, or
  //      the assistant text got rewritten. Other guard flags
  //      (intent_detected, engagement_refused, constraint_lifted)
  //      are trace events that don't themselves require review.
  //
  // Acknowledged flags are filtered server-side by
  // `flag_kinds_by_conversation`, so the kinds list here is
  // already pruned; explicit ack drops a conversation out of
  // this tab without any client-side bookkeeping. Same applies
  // to acknowledged downvotes via the `unaddressed_down`
  // server-side accounting.
  const needsReview = (convId: string): boolean => {
    const kinds = flagKinds?.[convId] || []
    return (
      kinds.includes("extraction_constraint_activated") ||
      kinds.includes("extraction_rewrote")
    )
  }

  const displayConversations = useMemo(() => {
    let list = topicConvIds
      ? (conversations || []).filter((c) => topicConvIds.has(c.id))
      : (conversations || [])

    if (activeTab === "flagged") {
      list = list
        .filter((c) => (c.unaddressed_down ?? 0) > 0 || needsReview(c.id))
        // Sort: extraction-flagged conversations float to the top
        // (those are the ones that need a fresh teacher look),
        // then by unaddressed downvote count desc for the rest.
        .sort((a, b) => {
          const ax = needsReview(a.id) ? 1 : 0
          const bx = needsReview(b.id) ? 1 : 0
          if (ax !== bx) return bx - ax
          return (b.unaddressed_down ?? 0) - (a.unaddressed_down ?? 0)
        })
    } else if (activeTab === "unreviewed") {
      // Conversations the teaching team hasn't looked at since
      // the last student turn. Independent from "flagged"; a
      // conversation can be unreviewed without being flagged
      // (just a fresh student message with no extraction trip or
      // downvote), and vice versa (an acked flag stays out of
      // here once a teacher opens the row).
      list = list.filter((c) => c.teacher_unreviewed === true)
    }
    return list
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [conversations, topicConvIds, activeTab, flagKinds])

  const flaggedCount = useMemo(
    () =>
      (conversations || []).filter(
        (c) => (c.unaddressed_down ?? 0) > 0 || needsReview(c.id),
      ).length,
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [conversations, flagKinds],
  )

  const unreviewedCount = useMemo(
    () => (conversations || []).filter((c) => c.teacher_unreviewed === true).length,
    [conversations],
  )

  const grouped = new Map<string, { label: string; conversations: ConversationWithUser[] }>()
  for (const conv of displayConversations) {
    const key = conv.user_id
    if (!grouped.has(key)) {
      grouped.set(key, {
        label: conv.user_display_name || conv.user_eppn || t("shared.unknownUser"),
        conversations: [],
      })
    }
    grouped.get(key)!.conversations.push(conv)
  }

  return (
    <div className="space-y-4">
      {feedbackStats && (feedbackStats.total_up > 0 || feedbackStats.total_down > 0) && (
        <Card>
          <CardHeader>
            <CardTitle className="text-base">{t("conversations.feedbackTitle")}</CardTitle>
          </CardHeader>
          <CardContent className="space-y-3">
            <div className="flex items-center gap-6 text-sm">
              <div>
                <span className="text-2xl font-semibold">{feedbackStats.total_up}</span>
                <span className="ml-1.5 text-muted-foreground">{t("conversations.helpful")}</span>
              </div>
              <div>
                <span className="text-2xl font-semibold">{feedbackStats.total_down}</span>
                <span className="ml-1.5 text-muted-foreground">{t("conversations.flagged")}</span>
              </div>
            </div>
            {feedbackStats.categories.length > 0 && (
              <div className="flex flex-wrap gap-2">
                {feedbackStats.categories.map((c) => (
                  <Badge key={c.category ?? "null"} variant="secondary" className="text-xs font-normal">
                    {categoryLabel(c.category)} &middot; {c.count}
                  </Badge>
                ))}
              </div>
            )}
          </CardContent>
        </Card>
      )}

      {!topicsLoading && topics && topics.length > 0 && (
        <Card>
          <CardHeader>
            <CardTitle className="text-base">{t("conversations.popularTopicsTitle")}</CardTitle>
            <CardDescription>
              {t("conversations.popularTopicsDescription")}
            </CardDescription>
          </CardHeader>
          <CardContent className="space-y-4">
            <div className="flex flex-wrap items-center gap-3">
              <Select
                value={selectedTopic ?? ""}
                onValueChange={(v) => setSelectedTopic(v || null)}
              >
                <SelectTrigger className="w-full sm:w-72">
                  <SelectValue placeholder={t("conversations.topicPlaceholder")} />
                </SelectTrigger>
                <SelectContent>
                  {topics.map((topic) => (
                    <SelectItem key={topic.topic} value={topic.topic}>
                      {t("conversations.topicOption", { topic: topic.topic, convos: topic.conversation_count, users: topic.unique_users })}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
              {selectedTopic && (
                <Button
                  variant="ghost"
                  size="sm"
                  onClick={() => setSelectedTopic(null)}
                >
                  {t("conversations.clearFilter")}
                </Button>
              )}
            </div>
            {activeTopic && (
              <div className="text-sm text-muted-foreground">
                {t("conversations.topicStats", { convos: activeTopic.conversation_count, users: activeTopic.unique_users, messages: activeTopic.total_messages })}
              </div>
            )}
          </CardContent>
        </Card>
      )}
      {topicsLoading && (
        <Card>
          <CardHeader>
            <Skeleton className="h-5 w-40" />
            <Skeleton className="h-4 w-64 mt-1" />
          </CardHeader>
          <CardContent>
            <Skeleton className="h-10 w-full sm:w-72" />
          </CardContent>
        </Card>
      )}
      <Card>
        <CardHeader>
          <CardTitle>
            {t("conversations.studentConversations")}
            {activeTopic && (
              <Badge variant="secondary" className="ml-2 font-normal">
                {t("conversations.filteredPrefix", { topic: activeTopic.topic })}
              </Badge>
            )}
          </CardTitle>
          <CardDescription>
            {t("conversations.conversationsDescription")}
          </CardDescription>
        </CardHeader>
        <CardContent className="space-y-4">
          <div className="flex gap-2">
            <Button
              variant={activeTab === "all" ? "default" : "outline"}
              size="sm"
              onClick={() => setActiveTab("all")}
            >
              {t("conversations.tabAll")}
            </Button>
            <Button
              variant={activeTab === "flagged" ? "default" : "outline"}
              size="sm"
              onClick={() => setActiveTab("flagged")}
            >
              {t("conversations.tabFlagged")}
              {flaggedCount > 0 && (
                <Badge variant="destructive" className="ml-1.5 px-1.5 py-0 text-xs">
                  {flaggedCount}
                </Badge>
              )}
            </Button>
            <Button
              variant={activeTab === "unreviewed" ? "default" : "outline"}
              size="sm"
              onClick={() => setActiveTab("unreviewed")}
            >
              {t("conversations.tabUnreviewed")}
              {unreviewedCount > 0 && (
                <Badge variant="secondary" className="ml-1.5 px-1.5 py-0 text-xs">
                  {unreviewedCount}
                </Badge>
              )}
            </Button>
          </div>

          {isLoading && (
            <div className="space-y-2">
              <Skeleton className="h-10 w-full" />
              <Skeleton className="h-10 w-full" />
              <Skeleton className="h-10 w-full" />
            </div>
          )}
          {!isLoading && displayConversations.length === 0 && (
            <p className="text-muted-foreground text-sm">
              {activeTab === "flagged"
                ? t("conversations.emptyFlagged")
                : activeTab === "unreviewed"
                  ? t("conversations.emptyUnreviewed")
                  : activeTopic
                    ? t("conversations.emptyTopic")
                    : t("conversations.emptyAll")}
            </p>
          )}
          <div className="space-y-6">
            {Array.from(grouped.entries()).map(([userId, group]) => (
              <div key={userId}>
                <h4 className="font-medium text-sm mb-2">{group.label}</h4>
                <div className="space-y-1">
                  {group.conversations.map((conv) => {
                    const expanded = expandedId === conv.id
                    const panelId = `conv-panel-${conv.id}`
                    return (
                      <div key={conv.id}>
                        <div
                          className={`flex items-center justify-between py-2 px-3 rounded ${
                            expanded ? "bg-secondary" : "hover:bg-muted"
                          }`}
                        >
                          <button
                            type="button"
                            onClick={() => {
                              const next = expanded ? null : conv.id
                              setExpandedId(next)
                              // Fire mark-reviewed when transitioning to
                              // expanded (not on collapse). Skip if the
                              // row is already reviewed so we don't churn
                              // the upsert + invalidate the list for no
                              // change.
                              if (next !== null && conv.teacher_unreviewed) {
                                markReviewedMutation.mutate(conv.id)
                              }
                            }}
                            aria-expanded={expanded}
                            aria-controls={panelId}
                            className="flex items-center gap-2 min-w-0 flex-1 text-left cursor-pointer focus-visible:outline-2 focus-visible:outline-ring focus-visible:outline-offset-2 rounded"
                          >
                            {conv.teacher_unreviewed && (
                              // Per-row unread dot for the teaching
                              // team. Same affordance as the student
                              // sidebar (`ConversationList`) so the
                              // surface reads uniformly across roles.
                              <span
                                aria-label={t("conversations.unreviewedDot")}
                                title={t("conversations.unreviewedDot")}
                                className="inline-block w-2 h-2 rounded-full bg-primary shrink-0"
                              />
                            )}
                            <span className="text-sm truncate">
                              {conv.title || t("conversations.untitled")}
                            </span>
                            <span className="text-xs text-muted-foreground shrink-0">
                              {t("conversations.msgsSuffix", { count: conv.message_count || 0 })}
                            </span>
                            {conv.pinned && (
                              <Badge variant="secondary" className="shrink-0">{t("conversations.pinned")}</Badge>
                            )}
                            {(conv.feedback_down ?? 0) > 0 && (
                              <Badge variant="outline" className="shrink-0 border-red-300 text-red-600 dark:border-red-700 dark:text-red-400 text-xs">
                                {t("conversations.flaggedBadge", { count: conv.feedback_down })}
                              </Badge>
                            )}
                            {(conv.feedback_up ?? 0) > 0 && (conv.feedback_down ?? 0) === 0 && (
                              <Badge variant="outline" className="shrink-0 border-green-300 text-green-600 dark:border-green-700 dark:text-green-400 text-xs">
                                {t("conversations.helpfulBadge", { count: conv.feedback_up })}
                              </Badge>
                            )}
                            {/* Extraction-guard list-view badges.
                                The append-only flag log emits
                                five kinds per lifecycle:
                                  - extraction_intent_detected
                                  - extraction_constraint_activated
                                  - extraction_engagement_refused
                                  - extraction_rewrote
                                  - extraction_constraint_lifted
                                For triage we badge only the two
                                that signal *something happened
                                that warrants a teacher's attention*
                                (constraint activated, output got
                                rewritten). The other three are
                                trace events that show the full
                                lifecycle on the detail page but
                                would be noise here. */}
                            {(flagKinds?.[conv.id] || [])
                              .filter(
                                (k) =>
                                  k === "extraction_constraint_activated" ||
                                  k === "extraction_rewrote",
                              )
                              .map((kind) => (
                                <Badge
                                  key={kind}
                                  variant="outline"
                                  className="shrink-0 border-amber-300 text-amber-700 dark:border-amber-600 dark:text-amber-400 text-xs"
                                >
                                  {t(`conversations.flagKind.${kind}`, kind)}
                                </Badge>
                              ))}
                          </button>
                          <div className="flex items-center gap-2 shrink-0 ml-2">
                            <span className="text-xs text-muted-foreground">
                              <RelativeTime date={conv.updated_at} />
                            </span>
                            <Button
                              variant={conv.pinned ? "default" : "outline"}
                              size="sm"
                              onClick={() => pinMutation.mutate({ cid: conv.id, pinned: !conv.pinned })}
                            >
                              {conv.pinned ? t("conversations.unpin") : t("conversations.pin")}
                            </Button>
                          </div>
                        </div>
                        {expanded && (
                          <div id={panelId}>
                            <ConversationExpanded courseId={courseId} conversationId={conv.id} />
                          </div>
                        )}
                      </div>
                    )
                  })}
                </div>
              </div>
            ))}
          </div>
        </CardContent>
      </Card>
    </div>
  )
}

function FeedbackBadges({
  courseId,
  conversationId,
  feedback,
}: {
  courseId: string
  conversationId: string
  feedback: MessageFeedback[]
}) {
  const { t } = useTranslation("teacher")
  const categoryLabel = useCategoryLabel()
  const queryClient = useQueryClient()
  // Per-row ack mutation. Course-shared (same semantics as flag
  // ack); whichever teacher clicks first resolves it for the
  // team. Symmetric with the legacy "leaving a note on this
  // message resolves it" rule; the dashboard's unaddressed_down
  // counter ORs both clearing paths so either drops the row out
  // of the "Flagged" tab.
  //
  // Declared BEFORE the early-return below so hooks are called in
  // a stable order on every render (rules-of-hooks).
  const ackFeedback = useMutation({
    mutationFn: (fbId: string) =>
      api.post(
        `/courses/${courseId}/conversations/${conversationId}/feedback/${fbId}/acknowledge`,
        {},
      ),
    onSuccess: () => {
      queryClient.invalidateQueries({
        queryKey: ["courses", courseId, "conversations", conversationId],
      })
      queryClient.invalidateQueries({
        queryKey: ["courses", courseId, "conversations", "all"],
      })
    },
  })
  const down = feedback.filter((f) => f.rating === "down")
  const up = feedback.filter((f) => f.rating === "up")
  if (down.length === 0 && up.length === 0) return null

  return (
    <div className="flex flex-wrap items-center gap-1 mt-1.5">
      {up.length > 0 && (
        <Badge variant="outline" className="text-xs border-green-300 text-green-700 dark:border-green-700 dark:text-green-300">
          {t("conversations.helpfulBadge", { count: up.length })}
        </Badge>
      )}
      {down.map((f) => {
        const isAcked = !!f.acknowledged_at
        return (
          <span key={f.id} className="inline-flex items-center gap-1.5">
            <Badge
              variant="outline"
              className={`text-xs ${
                isAcked
                  ? "border-muted-foreground/30 text-muted-foreground opacity-70"
                  : "border-red-300 text-red-700 dark:border-red-700 dark:text-red-300"
              }`}
              title={f.comment ?? undefined}
            >
              {categoryLabel(f.category)}
              {f.user_display_name ? ` (${f.user_display_name})` : ""}
            </Badge>
            {!isAcked && (
              <button
                type="button"
                className="text-xs text-red-700 dark:text-red-300 hover:underline"
                onClick={() => ackFeedback.mutate(f.id)}
                disabled={ackFeedback.isPending}
              >
                {t("conversations.acknowledgeFeedback")}
              </button>
            )}
            {isAcked && (
              <span className="text-xs text-muted-foreground italic">
                {t("conversations.acknowledgedBy", {
                  name:
                    f.acknowledger_display_name ?? t("conversations.unknownAcker"),
                })}
              </span>
            )}
          </span>
        )
      })}
    </div>
  )
}

/**
 * Per-turn extraction-guard flag display. Renders a coloured
 * badge with a teacher-readable label and the classifier's
 * rationale verbatim. Title-attribute carries the raw metadata
 * JSON for power users who want the full payload.
 *
 * Colour coding mirrors the lifecycle:
 *   - amber: high-signal events that warrant attention --
 *     constraint just activated, or the assistant text was
 *     rewritten because the output check tripped.
 *   - green: a lift event (engagement was detected and the
 *     constraint relaxed; positive outcome).
 *   - muted/neutral: trace events (intent classifier said yes,
 *     student refused engagement). Useful for the lifecycle
 *     timeline but not themselves a "look at this" signal.
 *
 * Reviewable kinds (constraint_activated, rewrote) carry an
 * Acknowledge button when `acknowledged_at` is null. Acked flags
 * stay in the list (audit trail) with a dimmed "✓ acknowledged
 * by X" caption. Trace events don't expose the button; they're
 * not the kind of thing a teacher decides about, just a record.
 */
function ConversationFlagDisplay({
  courseId,
  conversationId,
  flag,
}: {
  courseId: string
  conversationId: string
  flag: ConversationFlag
}) {
  const { t } = useTranslation("teacher")
  const queryClient = useQueryClient()
  const ackable =
    flag.flag === "extraction_constraint_activated" ||
    flag.flag === "extraction_rewrote"
  const isAcked = !!flag.acknowledged_at
  const ackMutation = useMutation({
    mutationFn: () =>
      api.post(
        `/courses/${courseId}/conversations/${conversationId}/flags/${flag.id}/acknowledge`,
        {},
      ),
    onSuccess: () => {
      // Both the detail panel (for the ✓ caption) and the
      // course-level conversation list (for the badge + Flagged
      // tab counter) read flag state, so invalidate both.
      queryClient.invalidateQueries({
        queryKey: ["courses", courseId, "conversations", conversationId],
      })
      queryClient.invalidateQueries({
        queryKey: ["courses", courseId, "conversations", "flag-kinds"],
      })
      queryClient.invalidateQueries({
        queryKey: ["courses", courseId, "conversations", "all"],
      })
    },
  })

  const colour = (() => {
    if (isAcked) {
      // Acked rows dim to muted regardless of original kind so
      // the audit trail reads "resolved" at a glance.
      return "border-muted-foreground/30 text-muted-foreground opacity-70"
    }
    switch (flag.flag) {
      case "extraction_constraint_lifted":
        return "border-green-300 text-green-700 dark:border-green-700 dark:text-green-300"
      case "extraction_constraint_activated":
      case "extraction_rewrote":
        return "border-amber-300 text-amber-700 dark:border-amber-700 dark:text-amber-300"
      case "extraction_intent_detected":
      case "extraction_engagement_refused":
      default:
        return "border-muted-foreground/40 text-muted-foreground"
    }
  })()
  const label = t(`conversations.flagKind.${flag.flag}`, flag.flag)
  return (
    <div className="mt-1.5 flex flex-wrap items-start gap-2">
      <Badge
        variant="outline"
        className={`text-xs ${colour}`}
        title={flag.metadata ? JSON.stringify(flag.metadata, null, 2) : undefined}
      >
        {label}
      </Badge>
      {flag.rationale && (
        <span className="text-xs text-muted-foreground italic">
          {flag.rationale}
        </span>
      )}
      {ackable && !isAcked && (
        <button
          type="button"
          className="text-xs text-amber-700 dark:text-amber-300 hover:underline"
          onClick={() => ackMutation.mutate()}
          disabled={ackMutation.isPending}
        >
          {t("conversations.acknowledgeFlag")}
        </button>
      )}
      {isAcked && (
        <span className="text-xs text-muted-foreground italic">
          {t("conversations.acknowledgedBy", {
            name:
              flag.acknowledger_display_name ?? t("conversations.unknownAcker"),
          })}
        </span>
      )}
    </div>
  )
}

function ConversationExpanded({ courseId, conversationId }: { courseId: string; conversationId: string }) {
  const { data, isLoading } = useQuery(conversationDetailQuery(courseId, conversationId))
  const { t } = useTranslation("teacher")
  const { t: tCommon } = useTranslation("common")
  const categoryLabel = useCategoryLabel()
  const queryClient = useQueryClient()
  const [noteContent, setNoteContent] = useState("")
  const [noteForMessage, setNoteForMessage] = useState<string | null>(null)

  const addNoteMutation = useMutation({
    mutationFn: (body: { content: string; message_id?: string }) =>
      api.post<TeacherNote>(`/courses/${courseId}/conversations/${conversationId}/notes`, body),
    onSuccess: () => {
      queryClient.invalidateQueries({
        queryKey: ["courses", courseId, "conversations", conversationId],
      })
      setNoteContent("")
      setNoteForMessage(null)
    },
  })

  const deleteNoteMutation = useMutation({
    mutationFn: (noteId: string) =>
      api.delete(`/courses/${courseId}/conversations/${conversationId}/notes/${noteId}`),
    onSuccess: () => {
      queryClient.invalidateQueries({
        queryKey: ["courses", courseId, "conversations", conversationId],
      })
    },
  })

  const openNoteForFeedback = (messageId: string, feedback: MessageFeedback[]) => {
    const down = feedback.filter((f) => f.rating === "down")
    const categories = [...new Set(down.map((f) => categoryLabel(f.category)))].join(", ")
    const prefix = categories
      ? t("conversations.correctionPrefixWithCategories", { categories })
      : t("conversations.correctionPrefix")
    setNoteForMessage(messageId)
    setNoteContent(prefix)
  }

  if (isLoading) {
    return (
      <div className="ml-4 border-l-2 pl-4 py-2 space-y-2">
        <Skeleton className="h-16 w-full" />
        <Skeleton className="h-16 w-full" />
      </div>
    )
  }

  const messages = data?.messages || []
  const notes = data?.notes || []
  const feedback = data?.feedback || []
  const flags = data?.flags || []

  const notesByMessage = new Map<string, TeacherNote[]>()
  const conversationNotes: TeacherNote[] = []
  for (const note of notes) {
    if (note.message_id) {
      const existing = notesByMessage.get(note.message_id) || []
      existing.push(note)
      notesByMessage.set(note.message_id, existing)
    } else {
      conversationNotes.push(note)
    }
  }

  const feedbackByMessage = new Map<string, MessageFeedback[]>()
  for (const f of feedback) {
    const existing = feedbackByMessage.get(f.message_id) || []
    existing.push(f)
    feedbackByMessage.set(f.message_id, existing)
  }

  // Walk the message list once to assign each message its 1-based
  // turn index (= count of user messages up to and including this
  // one). Backend flags carry the same index, so this gives us a
  // direct join between flags and the assistant reply that closed
  // the turn; which is the message the badge attaches to in the
  // UI.
  const turnByMessageId = new Map<string, number>()
  let turnCounter = 0
  for (const m of messages) {
    if (m.role === "user") turnCounter++
    turnByMessageId.set(m.id, turnCounter)
  }
  const flagsByTurn = new Map<number, ConversationFlag[]>()
  for (const f of flags) {
    if (f.turn_index === null || f.turn_index === undefined) continue
    const existing = flagsByTurn.get(f.turn_index) || []
    existing.push(f)
    flagsByTurn.set(f.turn_index, existing)
  }

  const handleAddNote = (messageId?: string) => {
    if (!noteContent.trim()) return
    addNoteMutation.mutate({
      content: noteContent,
      message_id: messageId || undefined,
    })
  }

  return (
    <div className="ml-4 border-l-2 pl-4 py-2 space-y-3 max-h-[600px] overflow-y-auto">
      <div className="space-y-2">
        <Label className="text-xs">{t("conversations.generalNoteLabel")}</Label>
        <div className="flex gap-2">
          <Textarea
            value={noteForMessage === null ? noteContent : ""}
            onChange={(e) => { setNoteForMessage(null); setNoteContent(e.target.value) }}
            placeholder={t("conversations.generalNotePlaceholder")}
            rows={2}
            className="flex-1"
          />
          <Button
            size="sm"
            className="self-end"
            onClick={() => handleAddNote()}
            disabled={addNoteMutation.isPending || !noteContent.trim() || noteForMessage !== null}
          >
            {t("conversations.addNoteButton")}
          </Button>
        </div>
      </div>
      <Separator />

      {conversationNotes.map((note) => (
        <NoteDisplay key={note.id} note={note} onDelete={() => deleteNoteMutation.mutate(note.id)} />
      ))}

      {messages.map((msg) => {
        const msgFeedback = feedbackByMessage.get(msg.id) || []
        const hasDownFeedback = msgFeedback.some((f) => f.rating === "down")
        // Flags align to the user-message-indexed turn. We attach
        // them to the assistant reply that closed the turn (the
        // visible "this answer was modified" UX); showing them
        // on the user message would be wrong UX (the user message
        // wasn't itself altered).
        const turnIdx = turnByMessageId.get(msg.id)
        const turnFlags =
          msg.role === "assistant" && turnIdx !== undefined
            ? flagsByTurn.get(turnIdx) || []
            : []
        return (
          <React.Fragment key={msg.id}>
            <div
              className={`rounded px-3 py-2 text-sm ${
                msg.role === "user" ? "bg-primary/10" : "bg-muted"
              }`}
            >
              <span className="text-xs font-medium text-muted-foreground block mb-1">
                {msg.role === "user" ? t("conversations.roleStudent") : t("conversations.roleAssistant")}
              </span>
              {msg.role === "user" ? (
                <p className="whitespace-pre-wrap">{msg.content}</p>
              ) : (
                <div className="prose prose-sm dark:prose-invert max-w-none">
                  <Markdown remarkPlugins={[remarkGfm]}>{msg.content}</Markdown>
                </div>
              )}
              {msg.role === "assistant" && msgFeedback.length > 0 && (
                <FeedbackBadges
                  courseId={courseId}
                  conversationId={conversationId}
                  feedback={msgFeedback}
                />
              )}
              {turnFlags.map((f) => (
                <ConversationFlagDisplay
                  key={f.id}
                  courseId={courseId}
                  conversationId={conversationId}
                  flag={f}
                />
              ))}
              <div className="flex items-center gap-3 mt-1.5">
                <button
                  className="text-xs text-muted-foreground hover:text-foreground underline"
                  onClick={() => setNoteForMessage(noteForMessage === msg.id ? null : msg.id)}
                >
                  {t("conversations.addNoteLink")}
                </button>
                {msg.role === "assistant" && hasDownFeedback && noteForMessage !== msg.id && (
                  <button
                    className="text-xs text-red-600 hover:text-red-800 dark:text-red-400 dark:hover:text-red-200 underline"
                    onClick={() => openNoteForFeedback(msg.id, msgFeedback)}
                  >
                    {t("conversations.addCorrectionLink")}
                  </button>
                )}
              </div>
            </div>

            {notesByMessage.get(msg.id)?.map((note) => (
              <NoteDisplay key={note.id} note={note} onDelete={() => deleteNoteMutation.mutate(note.id)} />
            ))}

            {noteForMessage === msg.id && (
              <div className="flex gap-2">
                <Textarea
                  value={noteContent}
                  onChange={(e) => setNoteContent(e.target.value)}
                  placeholder={t("conversations.messageNotePlaceholder")}
                  rows={2}
                  className="flex-1"
                />
                <div className="flex flex-col gap-1">
                  <Button
                    size="sm"
                    onClick={() => handleAddNote(msg.id)}
                    disabled={addNoteMutation.isPending || !noteContent.trim()}
                  >
                    {tCommon("actions.save")}
                  </Button>
                  <Button
                    size="sm"
                    variant="ghost"
                    onClick={() => { setNoteForMessage(null); setNoteContent("") }}
                  >
                    {tCommon("actions.cancel")}
                  </Button>
                </div>
              </div>
            )}
          </React.Fragment>
        )
      })}
    </div>
  )
}

function NoteDisplay({ note, onDelete }: { note: TeacherNote; onDelete: () => void }) {
  const { t } = useTranslation("teacher")
  const { t: tCommon } = useTranslation("common")
  return (
    <div className="bg-amber-50 dark:bg-amber-950/30 border border-amber-200 dark:border-amber-800 rounded px-3 py-2">
      <div className="flex items-center justify-between mb-1">
        <div className="flex items-center gap-2">
          <Badge variant="outline" className="text-xs border-amber-300 dark:border-amber-700 text-amber-700 dark:text-amber-300">
            {t("conversations.teacherNote")}
          </Badge>
          {note.author_display_name && (
            <span className="text-xs text-muted-foreground">{note.author_display_name}</span>
          )}
        </div>
        <Button variant="ghost" size="sm" className="h-6 px-2 text-xs" onClick={onDelete}>
          {tCommon("actions.delete")}
        </Button>
      </div>
      <div className="prose prose-sm dark:prose-invert max-w-none">
        <Markdown remarkPlugins={[remarkGfm]}>{note.content}</Markdown>
      </div>
    </div>
  )
}
