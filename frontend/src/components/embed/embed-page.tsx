import { useCallback, useEffect, useState } from "react"
import { useTranslation } from "react-i18next"
import { Button } from "@/components/ui/button"
import { Menu, X } from "lucide-react"
import { useDocumentTitle } from "@/lib/use-document-title"
import type { ChatBubbleLabels } from "@/components/chat/chat-bubble"
import { ConversationList } from "@/components/chat/conversation-list"
import {
  ChatSurface,
  type ChatSurfaceAdapter,
  type ChatSurfaceLabels,
  type ChatSurfaceLayout,
} from "@/components/chat/chat-surface"
import type { PromptAnalysis, TeacherNote } from "@/lib/types"

//; Types for embed API responses --

interface EmbedCourse {
  id: string
  name: string
  description: string | null
  /**
   * Per-course feature flags resolved server-side. Mirrors the
   * Shibboleth `Course.feature_flags` shape so the iframe can gate
   * the same UI affordances (currently the aegis Feedback panel)
   * without redefining the type.
   */
  feature_flags: {
    course_kg: boolean
    aegis: boolean
  }
}

interface EmbedConversation {
  id: string
  course_id: string
  title: string | null
  created_at: string
  updated_at: string
  /**
   * Mirrors the Shibboleth `Conversation.has_unread_note`: true
   * when a teacher note attached to this conversation post-dates
   * the owner's last view. Drives the unread dot in the embed
   * sidebar (same `ConversationList` component as the regular
   * chat page).
   */
  has_unread_note?: boolean
}

/**
 * Pinned-by-teacher conversations carry author metadata so non-owners
 * can see whose chat the teacher highlighted. Mirrors the
 * `ConversationWithUserResponse` JSON shape from the backend.
 */
interface EmbedPinnedConversation extends EmbedConversation {
  user_id: string
  user_eppn: string | null
  user_display_name: string | null
  pinned: boolean
  message_count: number | null
}

interface EmbedMessage {
  id: string
  role: "user" | "assistant"
  content: string
  chunks_used: string[] | null
  model_used: string | null
  thinking_transcript: string | null
  tool_events: PersistedToolEvent[] | null
  thinking_ms: number | null
  /**
   * True when the extraction guard suppressed this turn's thinking
   * stream. Server-side gate on the embed conversation-detail
   * route fills this from `messages.thinking_hidden` ORed with the
   * pre-migration historical signal; the embed surface always
   * treats the viewer as owner so suppression is unconditional.
   */
  thinking_hidden: boolean
  created_at: string
}

interface PersistedToolEvent {
  name: string
  args?: unknown
  result_summary?: string
  result?: unknown
}

interface EmbedConversationDetail {
  messages: EmbedMessage[]
  notes: TeacherNote[]
  /**
   * Aegis prompt-coaching analyses, one per scored user turn. Same
   * shape as the Shibboleth route; empty when aegis is off for
   * the course or every turn so far soft-failed.
   */
  prompt_analyses: PromptAnalysis[]
}

interface EmbedMe {
  id: string
  eppn: string
  display_name: string | null
  role: "student" | "teacher" | "admin"
  privacy_acknowledged_at: string | null
  lti_client_id: string | null
}

/** Read query params from the URL. */
function useToken(): string | null {
  const [token] = useState(() => {
    const params = new URLSearchParams(window.location.search)
    return params.get("token")
  })
  return token
}


/** Thin wrapper around fetch for the embed API. */
async function embedGet<T>(path: string, token: string): Promise<T> {
  const sep = path.includes("?") ? "&" : "?"
  const res = await fetch(`/api/embed${path}${sep}token=${encodeURIComponent(token)}`)
  if (!res.ok) {
    const body = await res.json().catch(() => ({ error: res.statusText }))
    throw new Error(body.error || res.statusText)
  }
  return res.json()
}

async function embedPost<T>(path: string, token: string, body?: unknown): Promise<T> {
  const sep = path.includes("?") ? "&" : "?"
  const res = await fetch(`/api/embed${path}${sep}token=${encodeURIComponent(token)}`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(body ?? {}),
  })
  if (!res.ok) {
    const b = await res.json().catch(() => ({ error: res.statusText }))
    throw new Error(b.error || res.statusText)
  }
  return res.json()
}

//; Main page --

export function EmbedPage({ useParams }: { useParams: () => { courseId: string } }) {
  const { t } = useTranslation("auth")
  const { t: tCommon } = useTranslation("common")
  const { courseId } = useParams()
  const token = useToken()
  const [course, setCourse] = useState<EmbedCourse | null>(null)
  const [conversations, setConversations] = useState<EmbedConversation[]>([])
  // Pinned-by-teacher chats. Loaded alongside the user's own
  // conversations so the sidebar can surface them with attribution,
  // mirroring the regular Shibboleth chat page. Previously absent from
  // the embed view, leaving teacher pins invisible inside iframes.
  const [pinned, setPinned] = useState<EmbedPinnedConversation[]>([])
  const [activeConvId, setActiveConvId] = useState<string | null>(null)
  const [me, setMe] = useState<EmbedMe | null>(null)
  const [suggestedQuestions, setSuggestedQuestions] = useState<string[]>([])
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)
  const [sidebarOpen, setSidebarOpen] = useState(false)

  // Collapse the sidebar whenever the active conversation changes.
  // Adjust-state-on-prop-change during render is the React-docs-
  // sanctioned alternative to setState-in-effect; see
  // https://react.dev/learn/you-might-not-need-an-effect#adjusting-some-state-when-a-prop-changes
  const [prevActiveConvIdForSidebar, setPrevActiveConvIdForSidebar] =
    useState(activeConvId)
  if (activeConvId !== prevActiveConvIdForSidebar) {
    setPrevActiveConvIdForSidebar(activeConvId)
    setSidebarOpen(false)
  }

  const refreshConversations = useCallback(async () => {
    if (!token) return
    try {
      const convs = await embedGet<EmbedConversation[]>(
        `/course/${courseId}/conversations`,
        token,
      )
      setConversations(convs)
    } catch {
      // Silent refresh failure
    }
  }, [courseId, token])

  // Stamp `student_last_viewed_at = NOW()` on the embed side when
  // the user opens an existing conversation. Mirrors the Shibboleth
  // chat-page mark-read effect; the embed handler validates that
  // the embed-token user owns the conversation before stamping.
  // Best-effort; failure just leaves the dot up.
  useEffect(() => {
    if (!token || activeConvId === null) return
    void embedPost(`/course/${courseId}/conversations/${activeConvId}/mark-read`, token)
      .then(() => void refreshConversations())
      .catch(() => {
        // Cosmetic; ignore.
      })
  }, [activeConvId, courseId, token, refreshConversations])

  useDocumentTitle(course ? tCommon("pageTitles.embed", { course: course.name }) : null)

  // Load course, user, conversations, and any teacher-pinned chats on
  // mount. Pinned failures are tolerated; the rest of the page still
  // works without them. The missing-token case is handled by the
  // render-time early return below, not here, so this effect can
  // assume `token` is set.
  useEffect(() => {
    if (!token) return
    let cancelled = false
    ;(async () => {
      try {
        const [c, convs, m, pins, suggestions] = await Promise.all([
          embedGet<EmbedCourse>(`/course/${courseId}`, token),
          embedGet<EmbedConversation[]>(`/course/${courseId}/conversations`, token),
          embedGet<EmbedMe>(`/course/${courseId}/me`, token),
          embedGet<EmbedPinnedConversation[]>(
            `/course/${courseId}/conversations/pinned`,
            token,
          ).catch(() => [] as EmbedPinnedConversation[]),
          // Soft-failures collapse to no chips; greeting still renders.
          embedGet<{ questions: string[] }>(
            `/course/${courseId}/suggested-questions`,
            token,
          ).catch(() => ({ questions: [] as string[] })),
        ])
        if (cancelled) return
        setCourse(c)
        setConversations(convs)
        setMe(m)
        setPinned(pins)
        setSuggestedQuestions(suggestions.questions)
        // Deliberately leave `activeConvId` as null on first load,
        // mirroring the Shibboleth route's `/new` redirect. LTI
        // re-launches and iframe refreshes used to land on the
        // student's most recent chat (or a teacher pin), which
        // polluted the context window with whatever they were last
        // doing. The empty state below now greets the user and
        // surfaces the input directly; the sidebar still lists
        // every prior + pinned chat for one-click resume.
      } catch (e) {
        if (!cancelled) setError(e instanceof Error ? e.message : t("embed.failedToLoad"))
      } finally {
        if (!cancelled) setLoading(false)
      }
    })()
    return () => { cancelled = true }
  }, [courseId, token, t])

  const acknowledgePrivacy = async () => {
    if (!token) return
    await embedPost<{ ok: boolean }>(`/course/${courseId}/acknowledge-privacy`, token)
    setMe((prev) => (prev ? { ...prev, privacy_acknowledged_at: new Date().toISOString() } : prev))
  }

  const needsPrivacyAck = !!me && !me.privacy_acknowledged_at

  // "New chat" is a UI-only action: it just clears the active conversation
  // so EmbedChatWindow renders a blank state with the input ready. The
  // actual conversation row is created lazily when the user submits their
  // first message (see EmbedChatWindow.handleSubmit).
  const startNewConversation = () => {
    setActiveConvId(null)
  }

  if (!token) {
    return (
      <div className="flex items-center justify-center h-full bg-background text-foreground">
        <p className="text-destructive">{t("embed.missingToken")}</p>
      </div>
    )
  }

  if (loading) {
    return (
      <div className="flex items-center justify-center h-full bg-background text-foreground">
        <div className="flex gap-1">
          <div className="w-2 h-2 bg-muted-foreground/40 rounded-full animate-bounce [animation-delay:0ms]" />
          <div className="w-2 h-2 bg-muted-foreground/40 rounded-full animate-bounce [animation-delay:150ms]" />
          <div className="w-2 h-2 bg-muted-foreground/40 rounded-full animate-bounce [animation-delay:300ms]" />
        </div>
      </div>
    )
  }

  if (error && !course) {
    return (
      <div className="flex items-center justify-center h-full bg-background text-foreground">
        <p className="text-destructive">{error}</p>
      </div>
    )
  }

  // Teacher pin freezes the chat for EVERYONE, owner included.
  // Previously this also required the viewer to not own the
  // conv, which let students keep appending to their own chats
  // after a pin and broke the "vetted exemplar" contract.
  // Backend enforces the same rule via `conversation.pinned_frozen`.
  const isPinnedView =
    activeConvId !== null &&
    pinned.some((p) => p.id === activeConvId)

  return (
    <div className="relative flex h-full bg-background text-foreground">
      <Button
        variant="outline"
        size="sm"
        className="md:hidden absolute top-2 left-2 z-20"
        onClick={() => setSidebarOpen(true)}
        aria-label={t("embed.openConversations")}
      >
        <Menu className="w-4 h-4" />
      </Button>
      {sidebarOpen && (
        <button
          type="button"
          aria-label={t("embed.closeConversations")}
          className="md:hidden fixed inset-0 z-30 bg-background/60"
          onClick={() => setSidebarOpen(false)}
        />
      )}
      <div
        className={`${
          sidebarOpen
            ? "fixed inset-y-0 left-0 z-40 w-64 bg-background border-r flex flex-col p-3 md:static md:inset-auto md:w-56"
            : "hidden md:flex md:w-56 border-r flex-col p-3"
        }`}
      >
        <div className="md:hidden flex justify-end mb-2">
          <Button
            variant="ghost"
            size="sm"
            onClick={() => setSidebarOpen(false)}
            aria-label={t("embed.closeConversations")}
          >
            <X className="w-4 h-4" />
          </Button>
        </div>
        <Button size="sm" className="mb-3" onClick={startNewConversation}>
          {t("embed.newChat")}
        </Button>
        <div className="space-y-1 overflow-y-auto flex-1">
          <ConversationList
            conversations={conversations}
            pinned={pinned}
            activeConversationId={activeConvId}
            renderRow={({ conversationId: cid, className, children }) => (
              <button
                key={cid}
                onClick={() => setActiveConvId(cid)}
                className={className}
              >
                {children}
              </button>
            )}
            labels={{
              // Embed has no per-user pin feature, but the field is
              // optional on `SidebarConversation` so a never-true
              // string is harmless and keeps the contract uniform.
              pinned: t("embed.pinnedByTeacher"),
              newConversation: t("embed.untitledConversation"),
              conversation: t("embed.untitledConversation"),
              pinnedByTeacher: t("embed.pinnedByTeacher"),
              studentFallback: t("embed.studentFallback"),
              unreadNote: t("embed.unreadNote"),
            }}
          />
        </div>
        {course && (
          <div className="text-xs text-muted-foreground pt-2 border-t mt-2">
            {course.name}
          </div>
        )}
      </div>

      <div className="flex-1 flex flex-col min-w-0 pt-12 md:pt-0">
        <EmbedChatWindow
          courseId={courseId}
          conversationId={activeConvId}
          token={token}
          onMessageSent={refreshConversations}
          onConversationCreated={setActiveConvId}
          needsPrivacyAck={needsPrivacyAck}
          onAcknowledgePrivacy={acknowledgePrivacy}
          readOnly={isPinnedView}
          aegisEnabled={course?.feature_flags?.aegis === true}
          courseName={course?.name ?? null}
          displayName={me?.display_name ?? null}
          suggestedQuestions={suggestedQuestions}
        />
      </div>
    </div>
  )
}

//; Chat window --

/**
 * Embed-side ChatSurface wrapper. Owns the embed-specific bits
 * (token-in-body auth, manual `useState` data layer, `auth` i18n
 * namespace with a few `student` aegis strings borrowed) and hands
 * everything else to the shared `<ChatSurface>` in
 * `components/chat/chat-surface.tsx`.
 */
function EmbedChatWindow({
  courseId,
  conversationId,
  token,
  onMessageSent,
  onConversationCreated,
  needsPrivacyAck,
  onAcknowledgePrivacy,
  readOnly = false,
  aegisEnabled = false,
  courseName = null,
  displayName = null,
  suggestedQuestions = [],
}: {
  courseId: string
  conversationId: string | null
  token: string
  onMessageSent: () => void
  onConversationCreated: (id: string) => void
  needsPrivacyAck: boolean
  onAcknowledgePrivacy: () => Promise<void>
  /**
   * Pinned conversations a teacher highlighted are not the viewer's
   * own; hide the input + send button just like the regular chat page
   * does for shared pinned views.
   */
  readOnly?: boolean
  /**
   * When true, the chat lays out as [transcript, feedback panel]
   * and SSE `prompt_analysis` events are surfaced into the panel.
   * Resolved upstream from `course.feature_flags.aegis` so the
   * panel auto-hides on courses where the admin hasn't opted in.
   */
  aegisEnabled?: boolean
  // Greeting + chip strip data; threaded from the parent so the
  // embed-only `/me` + `/course` fetches stay where they already live.
  courseName?: string | null
  displayName?: string | null
  suggestedQuestions?: string[]
}) {
  const { t } = useTranslation("auth")
  // Aegis strings live in the student namespace (the panel itself
  // reads them too). The embed view borrows them rather than
  // duplicating the i18n surface for one set of buttons.
  const { t: tStudent } = useTranslation("student")
  const [messages, setMessages] = useState<EmbedMessage[]>([])
  const [notes, setNotes] = useState<TeacherNote[]>([])
  const [promptAnalyses, setPromptAnalyses] = useState<PromptAnalysis[]>([])
  const [loading, setLoading] = useState(true)

  // Load messages when conversation changes. When conversationId is null,
  // the user clicked "New chat" and no conv row exists yet; render an
  // empty thread with the input ready (lazy creation happens on first send).
  //
  // The synchronous resets (setMessages/setNotes/etc.) happen during
  // render via the adjust-state-on-prop-change pattern; the genuine
  // async fetch stays in the effect below. See
  // https://react.dev/learn/you-might-not-need-an-effect#adjusting-some-state-when-a-prop-changes
  const [prevConversationId, setPrevConversationId] = useState(conversationId)
  if (conversationId !== prevConversationId) {
    setPrevConversationId(conversationId)
    if (conversationId === null) {
      setMessages([])
      setNotes([])
      setPromptAnalyses([])
      setLoading(false)
    } else {
      setLoading(true)
    }
  }
  useEffect(() => {
    if (conversationId === null) return
    let cancelled = false
    embedGet<EmbedConversationDetail>(`/course/${courseId}/conversations/${conversationId}`, token)
      .then((data) => {
        if (!cancelled) {
          setMessages(data.messages)
          setNotes(data.notes ?? [])
          setPromptAnalyses(data.prompt_analyses ?? [])
          setLoading(false)
        }
      })
      .catch(() => {
        // The shared surface owns the SSE error line; load failures
        // here just leave the transcript empty. Cosmetic enough that
        // we don't surface a toast.
        if (!cancelled) setLoading(false)
      })
    return () => { cancelled = true }
  }, [courseId, conversationId, token])

  // ---- ChatSurface adapter ----

  const buildSendFetch = useCallback<
    ChatSurfaceAdapter<EmbedMessage>["buildSendFetch"]
  >(
    ({ content, existingConvId, analysisAtSend }) => {
      const url = existingConvId
        ? `/api/embed/course/${courseId}/conversations/${existingConvId}/message`
        : `/api/embed/course/${courseId}/conversations`
      return () =>
        fetch(url, {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          // Token rides in the body for the SSE POST: EventSource
          // can't add custom headers and the URL gets logged.
          body: JSON.stringify({
            content,
            token,
            prompt_analysis: analysisAtSend,
          }),
        })
    },
    [courseId, token],
  )

  const fetchLiveAnalysis = useCallback<
    ChatSurfaceAdapter<EmbedMessage>["fetchLiveAnalysis"]
  >(
    async (content, previousSuggestions, mode, signal) => {
      const res = await fetch(`/api/embed/course/${courseId}/aegis/analyze`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
          content,
          token,
          conversation_id: conversationId,
          mode,
          // Live-iteration context; mirrors chat-page. The server
          // slots these onto the current-draft trail entry so the
          // already-addressed check can drop kinds the analyzer
          // just coached on a near-identical earlier version of the
          // same draft.
          previous_suggestions: previousSuggestions,
        }),
        signal,
      })
      if (!res.ok) return null
      return (await res.json()) as PromptAnalysis | null
    },
    [courseId, conversationId, token],
  )

  const fetchRewrite = useCallback<
    ChatSurfaceAdapter<EmbedMessage>["fetchRewrite"]
  >(
    async (draft, selected, mode) => {
      try {
        const res = await fetch(`/api/embed/course/${courseId}/aegis/rewrite`, {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify({
            content: draft,
            token,
            suggestions: selected,
            mode,
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
    [courseId, token],
  )

  const onAfterSend = useCallback<
    ChatSurfaceAdapter<EmbedMessage>["onAfterSend"]
  >(
    async (landedConvId) => {
      // Reload from the server so the persisted assistant reply
      // (with metadata) replaces the optimistic streamed copy.
      try {
        const data = await embedGet<EmbedConversationDetail>(
          `/course/${courseId}/conversations/${landedConvId}`,
          token,
        )
        setMessages(data.messages)
        setNotes(data.notes ?? [])
        setPromptAnalyses(data.prompt_analyses ?? [])
      } catch {
        // Silent
      }
      onMessageSent()
    },
    [courseId, token, onMessageSent],
  )

  const bubbleLabels: ChatBubbleLabels = {
    sourceCount: (count) => t("embed.sources", { count }),
    unknownSource: t("embed.unknownSource"),
    sourceUnavailable: t("embed.sourceUnavailable"),
    // The embed view intentionally hides token-usage stats: the iframe
    // sits in front of students who don't need to see model accounting.
  }

  const labels: ChatSurfaceLabels = {
    bubble: bubbleLabels,
    thinking: {
      thinkingActive: t("embed.thinkingActive"),
      thinkingDone: t("embed.thinkingDone"),
      thinkingDoneWithDuration: t("embed.thinkingDoneWithDuration"),
      thinkingHidden: t("embed.thinkingHidden"),
      thinkingHiddenBody: t("embed.thinkingHiddenBody"),
      toolCallsAriaLabel: t("embed.toolCallsAriaLabel"),
    },
    assistantResponse: t("embed.assistantResponseLabel"),
    unknownError: t("embed.unknownError"),
    send: t("embed.send"),
    inputPlaceholder: t("embed.inputPlaceholder"),
    aegisChecking: tStudent("aegis.checking"),
    aegisSendAsIs: tStudent("aegis.sendAsIs"),
    aegisPendingTitle: tStudent("aegis.pendingTitle"),
    aegisLooksGoodTitle: tStudent("aegis.looksGoodTitle"),
    aegisEmptyTitle: tStudent("aegis.emptyTitle"),
    aegisShowPanel: tStudent("aegis.showPanel"),
    aegisShowPanelButton: tStudent("aegis.showPanelButton"),
    disclaimerBefore: t("embed.disclosurePrefix"),
    disclaimerLink: t("embed.disclosureLink"),
    disclaimerAfter: t("embed.disclosureSuffix"),
    teacherNote: t("embed.teacherNote"),
  }

  const layout: ChatSurfaceLayout = {
    outerGap: "gap-2",
    transcriptScroll: "px-4",
    inputBlock: "p-4",
    // Iframe canvas can't spare 320px below md, so the panel
    // switches to drawer earlier than the Shibboleth chat page
    // (which uses `lg`).
    aegisDrawerBreakpoint: "md",
    // Embed leaves conv-level notes inline; the sticky bar reads
    // oddly against Moodle's surrounding chrome.
    stickyConversationNotes: false,
  }

  const adapter: ChatSurfaceAdapter<EmbedMessage> = {
    courseId,
    conversationId,
    messages,
    notes,
    promptAnalyses,
    isLoading: loading,
    courseName,
    displayName,
    suggestions: suggestedQuestions,
    needsPrivacyAck,
    onAcknowledgePrivacy,
    buildSendFetch,
    fetchLiveAnalysis,
    fetchRewrite,
    onAfterSend,
    onConversationCreated,
    readOnly,
    aegisEnabled,
    labels,
    layout,
  }

  return <ChatSurface adapter={adapter} />
}
