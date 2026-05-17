import { useCallback, useEffect, useState } from "react"
import { useTranslation } from "react-i18next"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { Menu, X } from "lucide-react"
import { PrivacyAckBanner } from "@/components/privacy-ack"
import { useDocumentTitle } from "@/lib/use-document-title"
import { ChatTranscript } from "@/components/chat/chat-transcript"
import type { ChatBubbleLabels } from "@/components/chat/chat-bubble"
import { ConversationList } from "@/components/chat/conversation-list"
import { EmptyChatGreeting } from "@/components/chat/empty-chat-greeting"
import { TeacherNoteInline } from "@/components/chat/teacher-note-inline"
import { useChatStream } from "@/components/chat/use-chat-stream"
import { AegisFeedbackPanel } from "@/components/chat/aegis-feedback-panel"
import { AegisShieldFilled } from "@/components/icons/aegis-shield-filled"
import { AegisSuggestionsBanner } from "@/components/chat/aegis-suggestions-banner"
import { useAegisLiveAnalyzer } from "@/components/chat/use-aegis-live-analyzer"
import { useAegisMode } from "@/components/chat/use-aegis-mode"
import { useAegisPanelVisible } from "@/components/chat/use-aegis-panel-visible"
import type { AegisSuggestion, PromptAnalysis, TeacherNote } from "@/lib/types"

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
  created_at: string
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
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)
  const [sidebarOpen, setSidebarOpen] = useState(false)

  useEffect(() => {
    setSidebarOpen(false)
  }, [activeConvId])

  useDocumentTitle(course ? tCommon("pageTitles.embed", { course: course.name }) : null)

  // Load course, user, conversations, and any teacher-pinned chats on
  // mount. Pinned failures are tolerated; the rest of the page still
  // works without them.
  useEffect(() => {
    if (!token) {
      setError(t("embed.missingToken"))
      setLoading(false)
      return
    }
    let cancelled = false
    ;(async () => {
      try {
        const [c, convs, m, pins] = await Promise.all([
          embedGet<EmbedCourse>(`/course/${courseId}`, token),
          embedGet<EmbedConversation[]>(`/course/${courseId}/conversations`, token),
          embedGet<EmbedMe>(`/course/${courseId}/me`, token),
          embedGet<EmbedPinnedConversation[]>(
            `/course/${courseId}/conversations/pinned`,
            token,
          ).catch(() => [] as EmbedPinnedConversation[]),
        ])
        if (cancelled) return
        setCourse(c)
        setConversations(convs)
        setMe(m)
        setPinned(pins)
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

  const refreshConversations = async () => {
    if (!token) return
    try {
      const convs = await embedGet<EmbedConversation[]>(`/course/${courseId}/conversations`, token)
      setConversations(convs)
    } catch {
      // Silent refresh failure
    }
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
        <div
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
            }}
          />
        </div>
        {course && (
          <div className="text-xs text-muted-foreground pt-2 border-t mt-2">
            {course.name}
          </div>
        )}
      </div>

      <div className="flex-1 flex flex-col min-w-0 pl-12 md:pl-0">
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
        />
      </div>
    </div>
  )
}

//; Chat window --

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
  /**
   * Course name + viewer display name surfaced in the
   * empty-state greeting that renders when `conversationId` is
   * null (i.e. fresh-launched iframe, before the user has typed
   * anything). Threaded from the parent so the embed-only `/me`
   * + `/course` fetches stay where they already live.
   */
  courseName?: string | null
  displayName?: string | null
}) {
  const { t } = useTranslation("auth")
  // Aegis strings live in the student namespace (the panel itself
  // reads them too). The embed view borrows them rather than
  // duplicating the i18n surface for one set of buttons.
  const { t: tStudent } = useTranslation("student")
  const [messages, setMessages] = useState<EmbedMessage[]>([])
  const [notes, setNotes] = useState<TeacherNote[]>([])
  // Aegis analyses live in component state alongside `messages`
  // because the embed view doesn't run on React Query; we hand-
  // load conversation detail on every conversation change. Same
  // soft-fail-to-empty fallback the route uses on the server side.
  const [promptAnalyses, setPromptAnalyses] = useState<PromptAnalysis[]>([])
  const [loading, setLoading] = useState(true)
  const [input, setInput] = useState("")
  const stream = useChatStream(t("embed.unknownError"))
  const { send, reset, setError } = stream

  // Subject-expertise mode shared with the panel toggle (see
  // chat-page for the rationale). Read-only here; the setter
  // is the panel's concern.
  const [aegisMode] = useAegisMode()
  const [panelVisible, setPanelVisible] = useAegisPanelVisible()

  // Live aegis analyzer. Auth flow differs from the Shibboleth
  // chat: the embed token rides in the request body alongside the
  // content, since iframes can't ship cookies cross-origin and
  // EventSource doesn't allow custom headers (we mirror that
  // shape for plain JSON POSTs to keep the body contract uniform).
  const fetchLiveAnalysis = useCallback(
    async (
      content: string,
      previousSuggestions: AegisSuggestion[],
      signal: AbortSignal,
    ): Promise<PromptAnalysis | null> => {
      const res = await fetch(`/api/embed/course/${courseId}/aegis/analyze`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
          content,
          token,
          conversation_id: conversationId,
          mode: aegisMode,
          // Live-iteration context; mirrors chat-page. The server
          // slots these onto the current-draft trail entry so the
          // already-addressed check can drop kinds the analyzer
          // just coached on a near-identical earlier version of
          // the same draft.
          previous_suggestions: previousSuggestions,
        }),
        signal,
      })
      if (!res.ok) return null
      return (await res.json()) as PromptAnalysis | null
    },
    [courseId, conversationId, token, aegisMode],
  )
  const liveAnalyzer = useAegisLiveAnalyzer(
    input,
    aegisEnabled,
    fetchLiveAnalysis,
    // Mode is in the resetKey so toggling Beginner/Expert wipes
    // the cached verdict; otherwise the analyzer's draft-match
    // short-circuit would serve the previous mode's result. See
    // the chat-page comment for the full rationale.
    `${courseId}:${conversationId ?? "new"}:${aegisMode}`,
  )

  // Load messages when conversation changes. When conversationId is null,
  // the user clicked "New chat" and no conv row exists yet; render an
  // empty thread with the input ready (lazy creation happens on first send).
  // The live analyzer wipes its own cached verdict via its resetKey so
  // we don't poke it from here.
  useEffect(() => {
    let cancelled = false
    reset()
    setInput("")

    if (conversationId === null) {
      setMessages([])
      setNotes([])
      setPromptAnalyses([])
      setLoading(false)
      return
    }

    setLoading(true)
    embedGet<EmbedConversationDetail>(`/course/${courseId}/conversations/${conversationId}`, token)
      .then((data) => {
        if (!cancelled) {
          setMessages(data.messages)
          setNotes(data.notes ?? [])
          setPromptAnalyses(data.prompt_analyses ?? [])
          setLoading(false)
        }
      })
      .catch((e) => {
        if (!cancelled) {
          setError(e instanceof Error ? e.message : t("embed.failedToLoadMessages"))
          setLoading(false)
        }
      })

    return () => { cancelled = true }
    // `reset`/`setError` from useChatStream are stable enough; including
    // them would refire this on every render.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [courseId, conversationId, token, t])

  // Index notes the same way the regular chat page does: per-message
  // notes render right after that bubble; conversation-level notes
  // (no message_id) render once above the thread.
  const notesByMessage = new Map<string, TeacherNote[]>()
  const conversationNotes: TeacherNote[] = []
  for (const note of notes) {
    if (note.message_id) {
      const existing = notesByMessage.get(note.message_id) ?? []
      existing.push(note)
      notesByMessage.set(note.message_id, existing)
    } else {
      conversationNotes.push(note)
    }
  }

  /**
   * Returns the conversation id this send landed in (the existing one
   * for an append, or the server-assigned one for a brand-new conv
   * signaled via the first SSE event), or null if the send failed
   * before any conv was created.
   */
  const sendMessage = async (
    content: string,
    existingConvId: string | null,
  ): Promise<string | null> => {
    // Existing conv -> append endpoint. New conv -> course-level
    // create-with-message endpoint, which generates the id server-side
    // and returns it as the first SSE event.
    const url = existingConvId
      ? `/api/embed/course/${courseId}/conversations/${existingConvId}/message`
      : `/api/embed/course/${courseId}/conversations`

    // Snapshot the live analysis on submit so the panel state at
    // the moment of Send is what the History row records.
    const analysisAtSend = liveAnalyzer.consume()

    let landedConvId: string | null = existingConvId
    const ok = await send(
      content,
      () =>
        fetch(url, {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          // Token rides in the body for the SSE POST: EventSource can't
          // add custom headers and the URL gets logged.
          body: JSON.stringify({
            content,
            token,
            prompt_analysis: analysisAtSend,
          }),
        }),
      (data) => {
        if (data.type === "conversation_created" && typeof data.id === "string") {
          landedConvId = data.id
        }
      },
    )
    if (ok && landedConvId) {
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
    }
    return ok ? landedConvId : null
  }

  // Soft-block intercept (matches chat-page). The analyzer runs on
  // Send so fast typers who press Enter before the debounce fires
  // still get a chance to see suggestions; if any come back, the
  // Send button re-labels to "Send as-is" and a small inline note
  // appears. Pressing Send again with the same draft dispatches.
  // Resets when the input changes or after a successful send.
  const [confirmDraftSend, setConfirmDraftSend] = useState<string | null>(null)
  const [submitChecking, setSubmitChecking] = useState(false)

  useEffect(() => {
    if (confirmDraftSend !== null && confirmDraftSend !== input) {
      setConfirmDraftSend(null)
    }
  }, [input, confirmDraftSend])

  const dispatchSend = (msg: string) => {
    setInput("")
    setConfirmDraftSend(null)
    ;(async () => {
      const landedConvId = await sendMessage(msg, conversationId)
      if (landedConvId && conversationId === null) {
        onConversationCreated(landedConvId)
      }
    })()
  }

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault()
    if (!input.trim() || stream.streaming || submitChecking) return
    const msg = input

    if (confirmDraftSend === msg) {
      dispatchSend(msg)
      return
    }
    if (!aegisEnabled) {
      dispatchSend(msg)
      return
    }

    setSubmitChecking(true)
    const verdict = await liveAnalyzer.analyzeNow(msg)
    setSubmitChecking(false)
    if (verdict && verdict.suggestions.length > 0) {
      setConfirmDraftSend(msg)
      return
    }
    dispatchSend(msg)
  }

  const sendNeedsConfirm =
    confirmDraftSend !== null && confirmDraftSend === input

  // Banner state; mirrors chat-page. The rewrite call uses the
  // embed-token-in-body auth flow rather than cookies + dev-user.
  const [bannerDismissedFor, setBannerDismissedFor] = useState<string | null>(
    null,
  )
  const [rewriting, setRewriting] = useState(false)
  useEffect(() => {
    if (bannerDismissedFor !== null && bannerDismissedFor !== input) {
      setBannerDismissedFor(null)
    }
  }, [input, bannerDismissedFor])
  const liveSuggestions = liveAnalyzer.analysis?.suggestions ?? []
  const showBanner =
    aegisEnabled &&
    liveSuggestions.length > 0 &&
    bannerDismissedFor !== input

  // Preview-and-apply flow mirrors chat-page; see the long
  // commentary there for the rationale (TL;DR: pilot users
  // disliked the auto-rewrite-and-send "Use ideas" button; this
  // flow returns control by previewing the rewrite read-only and
  // letting the student apply it to the input themselves).
  const handlePreviewIdeas = async (
    selected: AegisSuggestion[],
  ): Promise<string | null> => {
    if (rewriting) return null
    const draft = input
    if (!draft.trim() || selected.length === 0) return null
    setRewriting(true)
    try {
      const res = await fetch(`/api/embed/course/${courseId}/aegis/rewrite`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
          content: draft,
          token,
          suggestions: selected,
          mode: aegisMode,
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
    } finally {
      setRewriting(false)
    }
  }

  const handleApplyRewrite = (rewritten: string) => {
    if (!rewritten.trim()) return
    setInput(rewritten)
    setConfirmDraftSend(rewritten)
    // Drop the cached old verdict so the banner hides naturally
    // during the ~400ms wait for the rewritten input's own analyze
    // call rather than briefly flashing the previous draft's
    // suggestions. Replaces the older `setBannerDismissedFor` call
    // here which suppressed the banner past apply even when the
    // new verdict had genuinely new ideas; see the chat-page
    // counterpart for the full rationale.
    liveAnalyzer.reset()
  }

  const bubbleLabels: ChatBubbleLabels = {
    sourceCount: (count) => t("embed.sources", { count }),
    unknownSource: t("embed.unknownSource"),
    sourceUnavailable: t("embed.sourceUnavailable"),
    // The embed view intentionally hides token-usage stats: the iframe
    // sits in front of students who don't need to see model accounting.
  }

  // Greeting hero in place of the transcript on a fresh iframe
  // launch (no conv selected, nothing pending or streaming). The
  // first send fills `pendingUserMsg` and the transcript takes
  // over from there.
  const showGreeting =
    conversationId === null && !stream.streaming && !stream.pendingUserMsg

  return (
    <div className="relative flex flex-1 min-h-0 gap-2">
      <div className="flex-1 flex flex-col min-w-0">
      <div className="flex-1 overflow-y-auto px-4">
        {showGreeting ? (
          <div className="h-full flex items-center justify-center">
            <EmptyChatGreeting
              displayName={displayName}
              courseName={courseName}
            />
          </div>
        ) : (
        <ChatTranscript<EmbedMessage>
          messages={messages}
          isLoading={loading}
          pendingUserMsg={stream.pendingUserMsg}
          streaming={stream.streaming}
          streamedTokens={stream.streamedTokens}
          error={stream.error}
          bubbleLabels={bubbleLabels}
          assistantResponseLabel={t("embed.assistantResponseLabel")}
          renderBeforeMessages={() =>
            conversationNotes.length > 0 ? (
              <div className="space-y-2">
                {conversationNotes.map((note) => (
                  <TeacherNoteInline
                    key={note.id}
                    note={note}
                    label={t("embed.teacherNote")}
                  />
                ))}
              </div>
            ) : null
          }
          renderAfterMessage={(msg) =>
            notesByMessage.get(msg.id)?.map((note) => (
              <TeacherNoteInline
                key={note.id}
                note={note}
                label={t("embed.teacherNote")}
              />
            ))
          }
        />
        )}
      </div>

      {!readOnly && (
        <div className="p-4 border-t space-y-2">
          {needsPrivacyAck && <PrivacyAckBanner onAcknowledge={onAcknowledgePrivacy} />}
          {showBanner && (
            <AegisSuggestionsBanner
              suggestions={liveSuggestions}
              blocked={sendNeedsConfirm}
              working={rewriting}
              onPreview={handlePreviewIdeas}
              onApply={handleApplyRewrite}
              onDismiss={() => setBannerDismissedFor(input)}
            />
          )}
          {aegisEnabled && !showBanner && (
            // Persistent status row above the input, present whenever
            // aegis is on and the suggestions banner isn't taking the
            // slot. Three states (pending / clean verdict / idle) so
            // the iframe student sees a clear "aegis is on" signal at
            // all times, including before the analyzer has anything
            // to say. See chat-page.tsx for the full rationale.
            <div
              role="status"
              aria-live="polite"
              className="flex items-center gap-2 rounded-md border bg-muted/40 px-3 py-2 text-xs text-muted-foreground"
            >
              <AegisShieldFilled
                className={`w-4 h-4 shrink-0 ${liveAnalyzer.pending ? "animate-pulse" : ""}`}
              />
              <span>
                {liveAnalyzer.pending
                  ? tStudent("aegis.pendingTitle")
                  : liveAnalyzer.analysis &&
                      liveAnalyzer.analysis.suggestions.length === 0
                    ? tStudent("aegis.looksGoodTitle")
                    : tStudent("aegis.emptyTitle")}
              </span>
            </div>
          )}
          <form onSubmit={handleSubmit} className="flex gap-2">
            <Input
              value={input}
              onChange={(e) => setInput(e.target.value)}
              placeholder={t("embed.inputPlaceholder")}
              disabled={stream.streaming || needsPrivacyAck}
              className="flex-1"
            />
            <Button
              type="submit"
              variant={sendNeedsConfirm ? "outline" : "default"}
              disabled={
                stream.streaming ||
                !input.trim() ||
                needsPrivacyAck ||
                submitChecking
              }
            >
              {submitChecking
                ? tStudent("aegis.checking")
                : sendNeedsConfirm
                  ? tStudent("aegis.sendAsIs")
                  : t("embed.send")}
            </Button>
          </form>
          <p className="text-xs text-muted-foreground text-center">
            {t("embed.disclosurePrefix")}
            <a href="/data-handling" target="_blank" rel="noopener noreferrer" className="underline hover:text-foreground">{t("embed.disclosureLink")}</a>
            {t("embed.disclosureSuffix")}
          </p>
        </div>
      )}
      </div>
      {aegisEnabled && panelVisible && (
        <>
          {/*
            Below-md backdrop for the drawer. The embed iframe is
            typically narrower than the Shibboleth chat, so the
            in-flow rail switches to a drawer earlier (md vs lg
            on chat-page).
          */}
          <div
            className="md:hidden fixed inset-0 z-30 bg-background/60"
            onClick={() => setPanelVisible(false)}
            aria-hidden="true"
          />
          {/*
            Right-rail Feedback panel. Two layouts driven off the
            same element:
              * md+   -> in-flow column to the right of the chat.
              * <md   -> fixed drawer from the right edge.
            Same component as the Shibboleth route to keep visual
            + behavioural parity; only the breakpoint differs
            (the iframe canvas can't spare 320px below md).
          */}
          <aside
            className="fixed inset-y-0 right-0 z-40 w-72 max-w-[90vw] bg-background border-l flex flex-col py-3 pr-3 md:static md:inset-auto md:z-auto md:w-72 md:max-w-none md:shrink-0 md:py-0 md:pr-0 md:bg-transparent"
          >
            <AegisFeedbackPanel
              analyses={promptAnalyses}
              onHide={() => setPanelVisible(false)}
            />
          </aside>
        </>
      )}
      {aegisEnabled && !panelVisible && (
        // "Bring Aegis back" pill. Renders at every breakpoint --
        // the panel adapts (drawer below md, in-flow rail at md+)
        // so a phone-width iframe student has the same affordance
        // as a desktop one. Pill chrome (bg, border, shadow,
        // label) so it reads as a real button.
        <button
          type="button"
          onClick={() => setPanelVisible(true)}
          className="absolute top-2 right-2 z-20 inline-flex items-center gap-2 rounded-full border bg-background px-3 py-1.5 text-xs font-medium shadow-sm hover:bg-muted/60 transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring/50"
          title={tStudent("aegis.showPanel")}
          aria-label={tStudent("aegis.showPanel")}
        >
          <AegisShieldFilled size={16} className="rounded-sm shrink-0" />
          <span>{tStudent("aegis.showPanelButton")}</span>
        </button>
      )}
    </div>
  )
}
