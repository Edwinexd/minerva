import { Link, useNavigate } from "@tanstack/react-router"
import { useQuery, useQueryClient } from "@tanstack/react-query"
import { useTranslation } from "react-i18next"
import {
  courseQuery,
  conversationsQuery,
  conversationDetailQuery,
  pinnedConversationsQuery,
  userQuery,
} from "@/lib/queries"
import { api } from "@/lib/api"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { Menu, X } from "lucide-react"
import React, { useCallback, useEffect, useMemo, useState } from "react"
import type {
  Message,
  MessageFeedback,
  PromptAnalysis,
  TeacherNote,
} from "@/lib/types"
import { FeedbackControls } from "@/components/message-feedback"
import { PrivacyAckBanner } from "@/components/privacy-ack"
import { useDocumentTitle } from "@/lib/use-document-title"
import { ChatTranscript } from "./chat-transcript"
import type { ChatBubbleLabels } from "./chat-bubble"
import { ConversationList } from "./conversation-list"
import { TeacherNoteInline } from "./teacher-note-inline"
import { useChatStream } from "./use-chat-stream"
import { AegisFeedbackPanel } from "./aegis-feedback-panel"
import { AegisShieldFilled } from "@/components/icons/aegis-shield-filled"
import { AegisSuggestionsBanner } from "./aegis-suggestions-banner"
import { useAegisLiveAnalyzer } from "./use-aegis-live-analyzer"
import { useAegisMode } from "./use-aegis-mode"
import { useAegisPanelVisible } from "./use-aegis-panel-visible"

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

  const isPinnedView = conversationId !== null &&
    pinned?.some((p) => p.id === conversationId) &&
    !conversations?.some((c) => c.id === conversationId)

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
        <div
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
            }}
          />
        </div>
        {course && (
          <div className="text-xs text-muted-foreground pt-2 border-t mt-2">
            {course.name}
          </div>
        )}
      </div>

      <div className="flex-1 flex flex-col min-w-0 pl-10 md:pl-0">
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

function ChatWindow({
  courseId,
  conversationId,
  readOnly = false,
  aegisEnabled = false,
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
}) {
  const navigate = useNavigate()
  const { t } = useTranslation("student")
  const { data, isLoading } = useQuery({
    ...conversationDetailQuery(courseId, conversationId ?? ""),
    enabled: conversationId !== null,
  })
  const messages = data?.messages
  const notes = data?.notes || []
  const feedback = data?.feedback || []
  // Memoise so the array identity is stable across renders --
  // the cleanup effect below depends on this list, and a fresh
  // `[]` literal each render would refire the effect every time.
  const promptAnalyses = useMemo(
    () => data?.prompt_analyses ?? [],
    [data?.prompt_analyses],
  )
  const { data: user } = useQuery(userQuery)
  const needsPrivacyAck = !!user && !user.privacy_acknowledged_at
  const queryClient = useQueryClient()

  // Build a map of message_id -> the current user's feedback row (if any)
  // so each ChatBubble knows whether to render thumbs as selected.
  const myFeedbackByMessage = new Map<string, MessageFeedback>()
  if (user) {
    for (const f of feedback) {
      if (f.user_id === user.id) myFeedbackByMessage.set(f.message_id, f)
    }
  }
  const [input, setInput] = useState("")
  const stream = useChatStream(t("chat.unknownError"))
  const { send, reset } = stream

  // Subject-expertise mode (Beginner/Expert). Read from the same
  // storage-backed hook the panel's toggle writes to, so flipping
  // the badge automatically affects the NEXT analyze call without
  // any prop wiring. We only need the value here -- the setter
  // lives in the panel.
  const [aegisMode] = useAegisMode()
  // Storage-backed; the X on the panel header writes false, the
  // floating Aegis logo button below brings it back. Default true
  // so a course with aegis on shows the feature by default.
  const [panelVisible, setPanelVisible] = useAegisPanelVisible()

  // Live aegis analyzer: hits the backend on debounced input
  // changes so the right-rail panel reflects the prompt the
  // student is currently composing -- BEFORE they hit Send.
  // The closure threads cookie auth + the dev-user header that
  // the rest of the chat path uses.
  const fetchLiveAnalysis = useCallback(
    async (
      content: string,
      signal: AbortSignal,
    ): Promise<PromptAnalysis | null> => {
      const devUser = localStorage.getItem("minerva-dev-user")
      const headers: Record<string, string> = {
        "Content-Type": "application/json",
      }
      if (devUser) headers["X-Dev-User"] = devUser
      const res = await fetch(`/api/courses/${courseId}/aegis/analyze`, {
        method: "POST",
        headers,
        body: JSON.stringify({
          content,
          conversation_id: conversationId,
          mode: aegisMode,
        }),
        signal,
      })
      if (!res.ok) return null
      // Server returns `null` directly when aegis is disabled or
      // the analyzer soft-failed. JSON parse handles both shapes.
      return (await res.json()) as PromptAnalysis | null
    },
    [courseId, conversationId, aegisMode],
  )
  const liveAnalyzer = useAegisLiveAnalyzer(
    input,
    aegisEnabled,
    fetchLiveAnalysis,
    // resetKey: conversation switches AND course changes both
    // wipe the cached verdict. Concatenated so distinct courses
    // can't ever share a cached verdict.
    `${courseId}:${conversationId ?? "new"}`,
  )

  // Index notes by message_id for inline display
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

  // Reset state when conversation changes. `liveAnalyzer` resets
  // its own cache via the resetKey above, so we don't poke it here.
  useEffect(() => {
    reset()
    setInput("")
    // `reset` from useChatStream is stable enough; including it would
    // refire the wipe on every render and clobber an in-flight stream
    // for the new id.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [conversationId])

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
      ? `/api/courses/${courseId}/conversations/${existingConvId}/message`
      : `/api/courses/${courseId}/conversations`

    // Snapshot the live analysis on submit -- the panel may
    // refresh asynchronously after this point, so we lock in
    // exactly what the student saw when they clicked Send. Server
    // persists this with the new message_id for the History panel.
    const analysisAtSend = liveAnalyzer.consume()

    let landedConvId: string | null = existingConvId
    const ok = await send(
      content,
      () => {
        const devUser = localStorage.getItem("minerva-dev-user")
        const headers: Record<string, string> = { "Content-Type": "application/json" }
        if (devUser) headers["X-Dev-User"] = devUser
        return fetch(url, {
          method: "POST",
          headers,
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
      (data) => {
        if (data.type === "conversation_created" && typeof data.id === "string") {
          landedConvId = data.id
        }
      },
    )
    if (landedConvId) {
      queryClient.invalidateQueries({
        queryKey: ["courses", courseId, "conversations", landedConvId],
      })
    }
    queryClient.invalidateQueries({
      queryKey: ["courses", courseId, "conversations"],
    })
    return ok ? landedConvId : null
  }

  // Soft-block state for the just-in-time intercept. When the
  // student presses Send AND aegis returns non-empty suggestions
  // for the current draft, we DON'T dispatch -- we set
  // `confirmDraftSend` to the draft string so the next press of
  // Send (with the same content) goes through. The Send button
  // re-labels to "Send as-is" + a small inline note appears under
  // the input. The right-rail panel is already showing the
  // suggestions, no popup, no modal.
  //
  // The analyzer runs on Send (`analyzeNow`) so a fast typer who
  // presses Enter inside the 1s debounce window still gets the
  // chance to see suggestions. `analyzeNow` short-circuits when
  // the cache already matches the exact input -- no second LLM
  // call for slow typers.
  //
  // Resets when the student edits the input or after a successful
  // send (so the same draft text typed-and-sent two turns later
  // gets a fresh analyzer pass).
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
        navigate({
          to: "/course/$courseId/$conversationId",
          params: { courseId, conversationId: landedConvId },
          replace: true,
        })
      }
    })()
  }

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault()
    if (!input.trim() || stream.streaming || submitChecking) return
    const msg = input

    // Second press of Send for the same draft we already
    // soft-blocked on -- the student saw the suggestions, decided
    // to send anyway. Dispatch immediately, no second analyzer
    // call (the verdict's already cached + visible).
    if (confirmDraftSend === msg) {
      dispatchSend(msg)
      return
    }

    // Aegis disabled for the course -> straight send, no checking.
    if (!aegisEnabled) {
      dispatchSend(msg)
      return
    }

    // First Send press with aegis on. Fire (or reuse cached)
    // analyzer. `analyzeNow` short-circuits if the cache already
    // matches `msg`, so a debounced verdict from earlier doesn't
    // cost a second LLM call. Otherwise we wait the ~250-500ms
    // analyzer round-trip with the button showing "Checking...".
    setSubmitChecking(true)
    const verdict = await liveAnalyzer.analyzeNow(msg)
    setSubmitChecking(false)

    if (verdict && verdict.suggestions.length > 0) {
      // Suggestions present -> soft-block. The student sees them
      // in the right rail; pressing Send again with the same draft
      // dispatches.
      setConfirmDraftSend(msg)
      return
    }

    // No suggestions (or analyzer soft-failed / aegis off) -> send.
    dispatchSend(msg)
  }

  const sendNeedsConfirm =
    confirmDraftSend !== null && confirmDraftSend === input

  // Banner state ("Aegis has N ideas" tile above the input). The
  // banner shows whenever the live verdict has suggestions for the
  // current draft AND the student hasn't dismissed it for THIS
  // draft. New input regenerates suggestions and clears the
  // dismissal so the banner can return.
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

  /**
   * "Some ideas" handler. POSTs the current draft + cached
   * suggestions to /aegis/rewrite, replaces the input with the
   * returned revision, and dispatches a Send. We bypass the
   * soft-block (the rewrite already incorporates the suggestions,
   * so re-blocking would loop) by setting confirmDraftSend to
   * the rewritten content before dispatching.
   */
  const handleUseIdeas = async () => {
    if (rewriting) return
    const draft = input
    if (!draft.trim() || liveSuggestions.length === 0) return
    setRewriting(true)
    try {
      const devUser = localStorage.getItem("minerva-dev-user")
      const headers: Record<string, string> = {
        "Content-Type": "application/json",
      }
      if (devUser) headers["X-Dev-User"] = devUser
      const res = await fetch(
        `/api/courses/${courseId}/aegis/rewrite`,
        {
          method: "POST",
          headers,
          body: JSON.stringify({
            content: draft,
            suggestions: liveSuggestions,
            mode: aegisMode,
          }),
        },
      )
      if (!res.ok) {
        // Soft fail -- leave the original input in place so the
        // student can still send. A 5xx logs the upstream error
        // server-side; we don't surface it (the rewrite is
        // optional).
        console.warn("aegis rewrite failed:", res.status)
        return
      }
      const body = (await res.json()) as { content: string }
      const rewritten = body.content?.trim() ?? ""
      if (!rewritten) return
      setInput(rewritten)
      setConfirmDraftSend(rewritten) // pre-confirm so the next dispatch isn't blocked
      dispatchSend(rewritten)
    } catch (e) {
      console.warn("aegis rewrite error:", e)
    } finally {
      setRewriting(false)
    }
  }

  const bubbleLabels: ChatBubbleLabels = {
    sourceCount: (count) => t("message.source", { count }),
    unknownSource: t("message.unknownSource"),
    sourceUnavailable: t("message.sourceUnavailable"),
    stats: {
      tokensUsed: (count) => t("message.tokensUsed", { count }),
      generationTime: (seconds) => t("message.generationTime", { seconds }),
      usingSuffix: t("message.usingSuffix"),
      acrossRetrievals: (count) => t("message.acrossRetrievals", { count }),
    },
  }

  return (
    <div className="relative flex flex-1 min-h-0 gap-4">
      <div className="flex-1 flex flex-col min-w-0">
      <div className="flex-1 overflow-y-auto pr-4">
        <ChatTranscript<Message>
          messages={messages}
          isLoading={isLoading}
          pendingUserMsg={stream.pendingUserMsg}
          streaming={stream.streaming}
          streamedTokens={stream.streamedTokens}
          error={stream.error}
          bubbleLabels={bubbleLabels}
          assistantResponseLabel={t("chat.assistantResponseLabel")}
          renderBeforeMessages={() =>
            conversationNotes.length > 0 ? (
              <div className="space-y-2">
                {conversationNotes.map((note) => (
                  <TeacherNoteInline
                    key={note.id}
                    note={note}
                    label={t("message.teacherNote")}
                  />
                ))}
              </div>
            ) : null
          }
          renderFeedbackSlot={(msg) =>
            !readOnly && msg.role === "assistant" ? (
              <FeedbackControls
                courseId={courseId}
                conversationId={conversationId!}
                messageId={msg.id}
                current={myFeedbackByMessage.get(msg.id) ?? null}
              />
            ) : null
          }
          renderAfterMessage={(msg) =>
            notesByMessage.get(msg.id)?.map((note) => (
              <TeacherNoteInline
                key={note.id}
                note={note}
                label={t("message.teacherNote")}
              />
            ))
          }
        />
      </div>

      {!readOnly && (
        <div className="pt-4 border-t space-y-2">
          {needsPrivacyAck && (
            <PrivacyAckBanner
              onAcknowledge={async () => {
                await api.post("/auth/acknowledge-privacy", {})
                await queryClient.invalidateQueries({ queryKey: ["auth", "me"] })
              }}
            />
          )}
          {showBanner && (
            <AegisSuggestionsBanner
              suggestionCount={liveSuggestions.length}
              blocked={sendNeedsConfirm}
              working={rewriting}
              onUseIdeas={handleUseIdeas}
              onDismiss={() => setBannerDismissedFor(input)}
            />
          )}
          <form onSubmit={handleSubmit} className="flex gap-2">
            <Input
              value={input}
              onChange={(e) => setInput(e.target.value)}
              placeholder={t("chat.inputPlaceholder")}
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
                ? t("aegis.checking")
                : sendNeedsConfirm
                  ? t("aegis.sendAsIs")
                  : t("chat.send")}
            </Button>
          </form>
          <p className="text-xs text-muted-foreground text-center">
            {t("chat.disclaimerBefore")}
            <Link to="/data-handling" className="underline hover:text-foreground">{t("chat.dataHandlingLink")}</Link>
            {t("chat.disclaimerAfter")}
          </p>
        </div>
      )}
      </div>
      {aegisEnabled && panelVisible && (
        <>
          {/*
            Below-lg backdrop. The panel renders as a fixed
            drawer at those sizes so the chat column keeps the
            room until the student opens it explicitly; the
            backdrop dismisses on tap, mirroring the
            conversations sidebar's mobile behaviour.
          */}
          <div
            className="lg:hidden fixed inset-0 z-30 bg-background/60"
            onClick={() => setPanelVisible(false)}
            aria-hidden="true"
          />
          {/*
            Right-rail Aegis panel. Two layouts driven off the
            same element so the visible/dismissed state stays
            consistent across breakpoints:
              * lg+   -> in-flow column to the right of the chat.
              * <lg   -> fixed drawer from the right edge.
            The panel's own X (onHide) closes both forms.
          */}
          <aside
            className="fixed inset-y-0 right-0 z-40 w-80 max-w-[90vw] bg-background border-l flex flex-col py-3 pr-3 lg:static lg:inset-auto lg:z-auto lg:w-80 lg:max-w-none lg:shrink-0 lg:py-0 lg:pr-0 lg:bg-transparent"
          >
            <AegisFeedbackPanel
              analyses={promptAnalyses}
              latest={liveAnalyzer.analysis}
              pending={liveAnalyzer.pending}
              onHide={() => setPanelVisible(false)}
            />
          </aside>
        </>
      )}
      {aegisEnabled && !panelVisible && (
        // "Bring Aegis back" pill. Renders at every breakpoint
        // now that the panel itself adapts (drawer below lg,
        // in-flow rail at lg+) -- a tablet user has the same
        // affordance as a desktop one. Pill chrome (bg, border,
        // shadow, label) so it reads as a real button rather
        // than a decorative icon floating in the chat column.
        <button
          type="button"
          onClick={() => setPanelVisible(true)}
          className="absolute top-2 right-2 z-20 inline-flex items-center gap-2 rounded-full border bg-background px-3 py-1.5 text-xs font-medium shadow-sm hover:bg-muted/60 transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring/50"
          title={t("aegis.showPanel")}
          aria-label={t("aegis.showPanel")}
        >
          <AegisShieldFilled size={16} className="rounded-sm shrink-0" />
          <span>{t("aegis.showPanelButton")}</span>
        </button>
      )}
    </div>
  )
}
