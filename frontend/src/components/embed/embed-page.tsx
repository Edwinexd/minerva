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
import { TeacherNoteInline } from "@/components/chat/teacher-note-inline"
import { useChatStream } from "@/components/chat/use-chat-stream"
import { AegisFeedbackPanel } from "@/components/chat/aegis-feedback-panel"
import { useAegisLiveAnalyzer } from "@/components/chat/use-aegis-live-analyzer"
import type { PromptAnalysis, TeacherNote } from "@/lib/types"

// -- Types for embed API responses --

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
   * shape as the Shibboleth route -- empty when aegis is off for
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

// -- Main page --

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
  // mount. Pinned failures are tolerated -- the rest of the page still
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
        if (convs.length > 0) {
          setActiveConvId(convs[0].id)
        } else if (pins.length > 0) {
          // Land on a pinned chat if the user has nothing of their own
          // -- otherwise the pane would be empty even though the
          // teacher highlighted something.
          setActiveConvId(pins[0].id)
        }
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

  // The active chat is a teacher pin the viewer doesn't own -> render
  // it read-only (hide the input below). Mirrors the regular page.
  const isPinnedView =
    activeConvId !== null &&
    pinned.some((p) => p.id === activeConvId) &&
    !conversations.some((c) => c.id === activeConvId)

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
        />
      </div>
    </div>
  )
}

// -- Chat window --

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
}) {
  const { t } = useTranslation("auth")
  const [messages, setMessages] = useState<EmbedMessage[]>([])
  const [notes, setNotes] = useState<TeacherNote[]>([])
  // Aegis analyses live in component state alongside `messages`
  // because the embed view doesn't run on React Query -- we hand-
  // load conversation detail on every conversation change. Same
  // soft-fail-to-empty fallback the route uses on the server side.
  const [promptAnalyses, setPromptAnalyses] = useState<PromptAnalysis[]>([])
  const [loading, setLoading] = useState(true)
  const [input, setInput] = useState("")
  const stream = useChatStream(t("embed.unknownError"))
  const { send, reset, setError } = stream

  // Live aegis analyzer. Auth flow differs from the Shibboleth
  // chat: the embed token rides in the request body alongside the
  // content, since iframes can't ship cookies cross-origin and
  // EventSource doesn't allow custom headers (we mirror that
  // shape for plain JSON POSTs to keep the body contract uniform).
  const fetchLiveAnalysis = useCallback(
    async (
      content: string,
      signal: AbortSignal,
    ): Promise<PromptAnalysis | null> => {
      const res = await fetch(`/api/embed/course/${courseId}/aegis/analyze`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
          content,
          token,
          conversation_id: conversationId,
        }),
        signal,
      })
      if (!res.ok) return null
      return (await res.json()) as PromptAnalysis | null
    },
    [courseId, conversationId, token],
  )
  const liveAnalyzer = useAegisLiveAnalyzer(
    input,
    aegisEnabled,
    fetchLiveAnalysis,
    `${courseId}:${conversationId ?? "new"}`,
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

  const handleSubmit = (e: React.FormEvent) => {
    e.preventDefault()
    if (!input.trim() || stream.streaming) return
    const msg = input
    setInput("")

    ;(async () => {
      const landedConvId = await sendMessage(msg, conversationId)
      if (landedConvId && conversationId === null) {
        onConversationCreated(landedConvId)
      }
    })()
  }

  const bubbleLabels: ChatBubbleLabels = {
    sourceCount: (count) => t("embed.sources", { count }),
    unknownSource: t("embed.unknownSource"),
    sourceUnavailable: t("embed.sourceUnavailable"),
    // The embed view intentionally hides token-usage stats: the iframe
    // sits in front of students who don't need to see model accounting.
  }

  return (
    <div className="flex flex-1 min-h-0 gap-2">
      <div className="flex-1 flex flex-col min-w-0">
      <div className="flex-1 overflow-y-auto px-4">
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
      </div>

      {!readOnly && (
        <div className="p-4 border-t space-y-2">
          {needsPrivacyAck && <PrivacyAckBanner onAcknowledge={onAcknowledgePrivacy} />}
          <form onSubmit={handleSubmit} className="flex gap-2">
            <Input
              value={input}
              onChange={(e) => setInput(e.target.value)}
              placeholder={t("embed.inputPlaceholder")}
              disabled={stream.streaming || needsPrivacyAck}
              className="flex-1"
            />
            <Button type="submit" disabled={stream.streaming || !input.trim() || needsPrivacyAck}>
              {t("embed.send")}
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
      {aegisEnabled && (
        // Right-rail Feedback panel. The embed canvas is typically
        // narrower than the Shibboleth chat, so the breakpoint is
        // tighter (md vs lg) -- on a small iframe the panel just
        // hides and the chat keeps the room. Same component as the
        // Shibboleth route to keep visual + behavioural parity.
        // Visible even on a brand-new (null) conversation so the
        // student sees feedback for their first prompt before
        // sending it.
        <aside className="hidden md:flex w-72 shrink-0 flex-col border-l">
          <AegisFeedbackPanel
            analyses={promptAnalyses}
            latest={liveAnalyzer.analysis}
            pending={liveAnalyzer.pending}
          />
        </aside>
      )}
    </div>
  )
}
