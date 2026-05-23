import { Link, useNavigate } from "@tanstack/react-router"
import { useQuery, useQueryClient } from "@tanstack/react-query"
import { useTranslation } from "react-i18next"
import {
  courseQuery,
  conversationsQuery,
  conversationDetailQuery,
  pinnedConversationsQuery,
  suggestedQuestionsQuery,
  userQuery,
} from "@/lib/queries"
import { api } from "@/lib/api"
import { Button } from "@/components/ui/button"
import { Menu, X } from "lucide-react"
import { useCallback, useEffect, useState } from "react"
import type { Message, MessageFeedback, PromptAnalysis } from "@/lib/types"
import { FeedbackControls } from "@/components/message-feedback"
import { useDocumentTitle } from "@/lib/use-document-title"
import type { ChatBubbleLabels } from "./chat-bubble"
import { ConversationList } from "./conversation-list"
import {
  ChatSurface,
  type ChatSurfaceAdapter,
  type ChatSurfaceLabels,
  type ChatSurfaceLayout,
} from "./chat-surface"
import type { AegisMode } from "./use-aegis-mode"

export function ChatRouteComponent({
  useParams,
}: {
  useParams: () => { courseId: string; conversationId: string }
}) {
  const { courseId, conversationId } = useParams()
  return <ChatPage courseId={courseId} conversationId={conversationId} />
}

export function NewChatRouteComponent({
  useParams,
}: {
  useParams: () => { courseId: string }
}) {
  const { courseId } = useParams()
  return <ChatPage courseId={courseId} conversationId={null} />
}

function ChatPage({
  courseId,
  conversationId,
}: {
  courseId: string
  conversationId: string | null
}) {
  const navigate = useNavigate()
  const { t } = useTranslation("student")
  const { t: tCommon } = useTranslation("common")
  const { data: course } = useQuery(courseQuery(courseId))
  const { data: conversations, isLoading: convLoading } = useQuery(conversationsQuery(courseId))
  const { data: pinned, isLoading: pinnedLoading } = useQuery(pinnedConversationsQuery(courseId))

  useDocumentTitle(course ? tCommon("pageTitles.course", { course: course.name }) : null)

  // A teacher pin freezes the chat for EVERYONE, the owner
  // included. Previously this also required the viewer to not
  // own the conv (`!conversations.some(...)`), which meant
  // students could still append to their own chats after a
  // pin; defeating the whole point of pinning ("this is the
  // vetted answer, don't change it"). Backend enforces the
  // same rule via `conversation.pinned_frozen`.
  const isPinnedView = conversationId !== null &&
    !!pinned?.some((p) => p.id === conversationId)

  const [sidebarOpen, setSidebarOpen] = useState(false)
  const [prevConversationId, setPrevConversationId] = useState(conversationId)

  // Close the sidebar whenever the active conversation changes.
  // Done during render (not in an effect) so React can batch it with the
  // parent render instead of triggering an extra cascade.
  if (prevConversationId !== conversationId) {
    setPrevConversationId(conversationId)
    setSidebarOpen(false)
  }

  return (
    <div className="relative flex h-[calc(100vh-120px)] gap-4">
      <Button
        variant="outline"
        size="sm"
        className="md:hidden absolute top-0 left-0 z-20"
        onClick={() => setSidebarOpen(true)}
        aria-label={t("sidebar.openConversations")}
      >
        <Menu className="w-4 h-4" />
      </Button>
      {sidebarOpen && (
        <button
          type="button"
          aria-label={t("sidebar.closeConversations")}
          className="md:hidden fixed inset-0 z-30 bg-background/60"
          onClick={() => setSidebarOpen(false)}
        />
      )}
      <div
        className={`${
          sidebarOpen
            ? "fixed inset-y-0 left-0 z-40 w-72 bg-background border-r p-4 flex flex-col md:static md:inset-auto md:w-64 md:p-0 md:pr-4 md:bg-transparent"
            : "hidden md:flex md:w-64 border-r pr-4 flex-col"
        }`}
      >
        <div className="md:hidden flex justify-end mb-2">
          <Button
            variant="ghost"
            size="sm"
            onClick={() => setSidebarOpen(false)}
            aria-label={t("sidebar.closeConversations")}
          >
            <X className="w-4 h-4" />
          </Button>
        </div>
        <Button
          className="mb-4"
          onClick={() => navigate({ to: "/course/$courseId/new", params: { courseId } })}
          disabled={conversationId === null}
        >
          {t("sidebar.newChat")}
        </Button>
        <div className="space-y-1 overflow-y-auto flex-1">
          <ConversationList
            conversations={conversations}
            conversationsLoading={convLoading}
            pinned={pinned}
            pinnedLoading={pinnedLoading}
            activeConversationId={conversationId}
            renderRow={({ conversationId: cid, className, children }) => (
              <Link
                key={cid}
                to="/course/$courseId/$conversationId"
                params={{ courseId, conversationId: cid }}
                className={className}
              >
                {children}
              </Link>
            )}
            labels={{
              pinned: t("sidebar.pinned"),
              newConversation: t("sidebar.newConversation"),
              conversation: t("sidebar.conversation"),
              pinnedByTeacher: t("sidebar.pinnedByTeacher"),
              studentFallback: t("sidebar.studentFallback"),
              unreadNote: t("sidebar.unreadNote"),
            }}
          />
        </div>
        {course && (
          <div className="text-xs text-muted-foreground pt-2 border-t mt-2">
            {course.name}
          </div>
        )}
      </div>

      <div className="flex-1 flex flex-col min-w-0 pt-10 md:pt-0">
        <ChatWindow
          courseId={courseId}
          conversationId={conversationId}
          readOnly={isPinnedView}
          aegisEnabled={course?.feature_flags?.aegis === true}
        />
      </div>
    </div>
  )
}

// Exported so the study mode's `<TaskRunner>` can reuse the full
// chat UX (transcript + composer + Aegis panel) without dragging
// in the conversation-list sidebar that ChatPage wraps it with.
// Study mode pins a per-task conversation_id and forces aegisEnabled
// true, so passing those through is enough; no other study-specific
// branching lives in here.
export function ChatWindow({
  courseId,
  conversationId,
  readOnly = false,
  aegisEnabled = false,
  forceAegisMode,
}: {
  courseId: string
  conversationId: string | null
  readOnly?: boolean
  /**
   * When true, the chat lays out as [transcript, feedback panel]
   * and SSE `prompt_analysis` events are surfaced into the panel.
   * Resolved upstream from `course.feature_flags.aegis` so the
   * panel auto-hides on courses where the admin hasn't opted in.
   */
  aegisEnabled?: boolean
  /**
   * When set, the Aegis analyzer's calibration mode is locked to
   * this value for the duration of this chat window; the panel's
   * Beginner/Expert toggle is disabled and the user's stored
   * preference is ignored. Study mode pins this to "expert" so
   * every participant runs under the same rubric (otherwise prior
   * localStorage values from regular chat use would inject mode
   * variance into the eval data).
   */
  forceAegisMode?: AegisMode
}) {
  const navigate = useNavigate()
  const { t } = useTranslation("student")
  // Course is already loaded by the parent ChatPage; React Query
  // dedups so this is a cache hit. Pulled in here (rather than
  // threaded as a prop) so study mode's TaskRunner caller doesn't
  // have to wire it through; study sessions always have a
  // non-null conversationId so the greeting block below never
  // fires for them anyway, and TaskRunner already has its own
  // task framing.
  const { data: course } = useQuery(courseQuery(courseId))
  // Only paid on /new; resuming an existing chat doesn't fetch.
  const { data: suggestions } = useQuery({
    ...suggestedQuestionsQuery(courseId),
    enabled: conversationId === null,
  })
  const { data, isLoading } = useQuery({
    ...conversationDetailQuery(courseId, conversationId ?? ""),
    enabled: conversationId !== null,
  })
  const messages = data?.messages
  const notes = data?.notes || []
  const feedback = data?.feedback || []
  const promptAnalyses = data?.prompt_analyses ?? []
  const { data: user } = useQuery(userQuery)
  const needsPrivacyAck = !!user && !user.privacy_acknowledged_at
  const queryClient = useQueryClient()

  // Build a map of message_id -> the current user's feedback row
  // (if any) so each ChatBubble knows whether to render thumbs as
  // selected.
  const myFeedbackByMessage = new Map<string, MessageFeedback>()
  if (user) {
    for (const f of feedback) {
      if (f.user_id === user.id) myFeedbackByMessage.set(f.message_id, f)
    }
  }

  // Mark the conversation as read on the student side whenever the
  // chat-page opens an existing conversation. Fire-and-forget; the
  // backend stamps `student_last_viewed_at = NOW()` so the sidebar's
  // unread dot (and the "My Courses" tile's unread badge) clear on
  // the next refetch. We invalidate the sidebar + the cross-course
  // rollup so the dot disappears without a full page reload.
  //
  // No mark-read for `conversationId === null` (i.e. /new); the
  // route hasn't created a row yet, and the empty-state has nothing
  // to mark read. We also skip readOnly contexts (study mode read-
  // only views) since those are pre-recorded research data, not the
  // user's own chat surface.
  useEffect(() => {
    if (conversationId === null || readOnly) return
    void api
      .post(`/courses/${courseId}/conversations/${conversationId}/mark-read`, {})
      .then(() => {
        queryClient.invalidateQueries({
          queryKey: ["courses", courseId, "conversations"],
        })
        queryClient.invalidateQueries({ queryKey: ["courses", "unread-counts"] })
      })
      .catch(() => {
        // Best-effort: a failed mark-read is purely cosmetic
        // (the dot just doesn't clear). Don't surface to the
        // user; logged at the network layer if relevant.
      })
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [conversationId, courseId, readOnly])

  // ---- ChatSurface adapter ----

  /**
   * Shared header builder for the Shibboleth auth flow: cookies
   * carry the Shibboleth session; the optional `X-Dev-User` header
   * unlocks the local dev-impersonation shortcut used in
   * `docker-compose.yml`. The embed route doesn't ship this header
   * because the iframe cookie boundary forces token-in-body auth
   * regardless of dev mode.
   */
  const buildHeaders = useCallback((): Record<string, string> => {
    const devUser = localStorage.getItem("minerva-dev-user")
    const headers: Record<string, string> = { "Content-Type": "application/json" }
    if (devUser) headers["X-Dev-User"] = devUser
    return headers
  }, [])

  const buildSendFetch = useCallback<ChatSurfaceAdapter<Message>["buildSendFetch"]>(
    ({ content, existingConvId, analysisAtSend }) => {
      const url = existingConvId
        ? `/api/courses/${courseId}/conversations/${existingConvId}/message`
        : `/api/courses/${courseId}/conversations`
      return () =>
        fetch(url, {
          method: "POST",
          headers: buildHeaders(),
          body: JSON.stringify({
            content,
            // Field is `Option<...>` server-side; omitting it (vs
            // sending `null`) is interchangeable thanks to
            // `#[serde(default)]` on the Rust side, but explicit
            // null reads clearer in the network panel.
            prompt_analysis: analysisAtSend,
          }),
        })
    },
    [courseId, buildHeaders],
  )

  const fetchLiveAnalysis = useCallback<
    ChatSurfaceAdapter<Message>["fetchLiveAnalysis"]
  >(
    async (content, previousSuggestions, mode, signal) => {
      const res = await fetch(`/api/courses/${courseId}/aegis/analyze`, {
        method: "POST",
        headers: buildHeaders(),
        body: JSON.stringify({
          content,
          conversation_id: conversationId,
          mode,
          // Live-iteration context: the suggestions Aegis returned on
          // the previous debounced fire of (a near-identical earlier
          // version of) this same draft. The server slots them onto
          // the current-draft trail entry so the already-addressed
          // check can drop kinds the analyzer just coached on;
          // without this the pre-Send loop is memoryless and pilot
          // users hit the "10 iterations and never happy" failure
          // mode.
          previous_suggestions: previousSuggestions,
        }),
        signal,
      })
      if (!res.ok) return null
      // Server returns `null` directly when aegis is disabled or the
      // analyzer soft-failed. JSON parse handles both shapes.
      return (await res.json()) as PromptAnalysis | null
    },
    [courseId, conversationId, buildHeaders],
  )

  const fetchRewrite = useCallback<
    ChatSurfaceAdapter<Message>["fetchRewrite"]
  >(
    async (draft, selected, mode) => {
      try {
        const res = await fetch(`/api/courses/${courseId}/aegis/rewrite`, {
          method: "POST",
          headers: buildHeaders(),
          body: JSON.stringify({
            content: draft,
            suggestions: selected,
            mode,
            // Lets the backend's per-conversation Aegis gate honour
            // study-mode off-rounds even when the umbrella flag is
            // forced on. Null is fine for brand-new composers (no
            // conv yet); the backend falls back to the umbrella in
            // that case.
            conversation_id: conversationId,
          }),
        })
        if (!res.ok) {
          console.warn("aegis rewrite failed:", res.status)
          return null
        }
        const body = (await res.json()) as { content: string }
        const rewritten = body.content?.trim() ?? ""
        return rewritten || null
      } catch (e) {
        console.warn("aegis rewrite error:", e)
        return null
      }
    },
    [courseId, conversationId, buildHeaders],
  )

  const onAfterSend = useCallback<ChatSurfaceAdapter<Message>["onAfterSend"]>(
    (landedConvId) => {
      queryClient.invalidateQueries({
        queryKey: ["courses", courseId, "conversations", landedConvId],
      })
      queryClient.invalidateQueries({
        queryKey: ["courses", courseId, "conversations"],
      })
    },
    [courseId, queryClient],
  )

  const onConversationCreated = useCallback(
    (id: string) => {
      navigate({
        to: "/course/$courseId/$conversationId",
        params: { courseId, conversationId: id },
        replace: true,
      })
    },
    [navigate, courseId],
  )

  const bubbleLabels: ChatBubbleLabels = {
    sourceCount: (count) => t("message.source", { count }),
    unknownSource: t("message.unknownSource"),
    sourceUnavailable: t("message.sourceUnavailable"),
    stats: {
      tokensUsed: (count) => t("message.tokensUsed", { count }),
      tokenBreakdown: (research, writeup) =>
        t("message.tokenBreakdown", { research, writeup }),
      generationTime: (seconds) => t("message.generationTime", { seconds }),
      usingSuffix: t("message.usingSuffix"),
      acrossRetrievals: (count) => t("message.acrossRetrievals", { count }),
    },
  }

  const labels: ChatSurfaceLabels = {
    bubble: bubbleLabels,
    thinking: {
      thinkingActive: t("chat.thinkingActive"),
      thinkingDone: t("chat.thinkingDone"),
      thinkingDoneWithDuration: t("chat.thinkingDoneWithDuration"),
      toolCallsAriaLabel: t("chat.toolCallsAriaLabel"),
    },
    assistantResponse: t("chat.assistantResponseLabel"),
    unknownError: t("chat.unknownError"),
    send: t("chat.send"),
    inputPlaceholder: t("chat.inputPlaceholder"),
    aegisChecking: t("aegis.checking"),
    aegisSendAsIs: t("aegis.sendAsIs"),
    aegisPendingTitle: t("aegis.pendingTitle"),
    aegisLooksGoodTitle: t("aegis.looksGoodTitle"),
    aegisEmptyTitle: t("aegis.emptyTitle"),
    aegisShowPanel: t("aegis.showPanel"),
    aegisShowPanelButton: t("aegis.showPanelButton"),
    disclaimerBefore: t("chat.disclaimerBefore"),
    disclaimerLink: t("chat.dataHandlingLink"),
    disclaimerAfter: t("chat.disclaimerAfter"),
    teacherNote: t("message.teacherNote"),
  }

  const layout: ChatSurfaceLayout = {
    outerGap: "gap-4",
    transcriptScroll: "pr-4",
    inputBlock: "pt-4",
    aegisDrawerBreakpoint: "lg",
    stickyConversationNotes: true,
  }

  const adapter: ChatSurfaceAdapter<Message> = {
    courseId,
    conversationId,
    messages,
    notes,
    promptAnalyses,
    isLoading,
    courseName: course?.name ?? null,
    displayName: user?.display_name ?? null,
    suggestions: suggestions?.questions,
    needsPrivacyAck,
    onAcknowledgePrivacy: async () => {
      await api.post("/auth/acknowledge-privacy", {})
      await queryClient.invalidateQueries({ queryKey: ["auth", "me"] })
    },
    buildSendFetch,
    fetchLiveAnalysis,
    fetchRewrite,
    onAfterSend,
    onConversationCreated,
    renderFeedbackSlot: (msg) => (
      <FeedbackControls
        courseId={courseId}
        conversationId={conversationId!}
        messageId={msg.id}
        current={myFeedbackByMessage.get(msg.id) ?? null}
      />
    ),
    readOnly,
    aegisEnabled,
    forceAegisMode,
    labels,
    layout,
  }

  return <ChatSurface adapter={adapter} />
}
