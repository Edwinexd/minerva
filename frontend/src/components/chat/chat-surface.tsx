/**
 * Shared chat surface used by the Shibboleth route (`ChatWindow` in
 * `chat-page.tsx`) and the LTI/embed route (`EmbedChatWindow` in
 * `embed-page.tsx`). Owns everything between (and including) the
 * transcript scroll area, the composer, the Aegis live-analyzer
 * intercept, the suggestions banner / preview-and-apply rewrite
 * flow, and the right-rail Aegis panel + bring-it-back pill.
 *
 * Everything that genuinely differs between the two routes is
 * pushed onto an adapter object:
 *
 *   * Auth / URL shape for the three I/O closures (`buildSendFetch`,
 *     `fetchLiveAnalysis`, `fetchRewrite`). Shibboleth uses cookies
 *     + an `X-Dev-User` header; embed ships its token in the body
 *     because iframes can't carry cross-origin cookies and
 *     EventSource can't add custom headers.
 *   * Data layer. Shibboleth feeds this from React Query caches;
 *     embed hand-rolls `useState` + `useEffect` because the embed
 *     auth model didn't justify pulling React Query into the iframe
 *     bundle. Both shapes resolve to `messages` / `notes` /
 *     `promptAnalyses` / `isLoading`, which is what this component
 *     consumes.
 *   * Conversation-creation hand-off. Shibboleth navigates to
 *     `/course/$courseId/$conversationId`; embed flips
 *     `setActiveConvId` and stays at the same iframe URL.
 *   * Post-send refresh. Shibboleth invalidates the React Query
 *     cache; embed reloads conversation detail via `embedGet`.
 *   * Labels. Shibboleth lives in the `student` i18n namespace;
 *     embed lives in `auth` (with a few aegis keys borrowed from
 *     `student`). Strings come in pre-translated.
 *   * Per-message extras. Shibboleth shows thumbs-up/down via
 *     `renderFeedbackSlot`; embed has no per-message feedback.
 *
 * Until this extraction the two routes carried near-identical
 * 400-line bodies and any change to the live-analyzer or soft-block
 * flow had to be made twice in lockstep. The mobile hamburger bug
 * `641031e`/`a780033` lived through several months precisely because
 * the embed copy never got the same `pl-* -> pt-*` flip.
 */
import React, { useCallback, useState } from "react"
import { Link } from "@tanstack/react-router"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { PrivacyAckBanner } from "@/components/privacy-ack"
import { AegisShieldFilled } from "@/components/icons/aegis-shield-filled"
import { ChatTranscript, type PersistedThinking } from "./chat-transcript"
import type { ChatBubbleLabels, ChatBubbleMessage } from "./chat-bubble"
import type { ThinkingBlockLabels } from "./thinking-block"
import { EmptyChatGreeting } from "./empty-chat-greeting"
import { TeacherNoteInline } from "./teacher-note-inline"
import { useChatStream } from "./use-chat-stream"
import { AegisFeedbackPanel } from "./aegis-feedback-panel"
import { AegisSuggestionsBanner } from "./aegis-suggestions-banner"
import { useAegisLiveAnalyzer } from "./use-aegis-live-analyzer"
import { useAegisMode, type AegisMode } from "./use-aegis-mode"
import { useAegisPanelVisible } from "./use-aegis-panel-visible"
import type {
  AegisSuggestion,
  PromptAnalysis,
  TeacherNote,
} from "@/lib/types"

/**
 * Strings the surface needs. Translation namespaces differ between
 * routes (`student` vs `auth`), so the adapter resolves them upstream
 * and hands a flat object in.
 */
export interface ChatSurfaceLabels {
  bubble: ChatBubbleLabels
  thinking: ThinkingBlockLabels
  assistantResponse: string
  // useChatStream's fallback when an SSE error has no message
  unknownError: string
  // Composer
  send: string
  inputPlaceholder: string
  // Aegis intercept + status
  aegisChecking: string
  aegisSendAsIs: string
  aegisPendingTitle: string
  aegisLooksGoodTitle: string
  aegisEmptyTitle: string
  aegisShowPanel: string
  aegisShowPanelButton: string
  // Privacy disclaimer (rendered as: before + link + after)
  disclaimerBefore: string
  disclaimerLink: string
  disclaimerAfter: string
  // Teacher notes
  teacherNote: string
}

/**
 * Layout knobs. Held apart from the rest of the adapter so callers
 * can pass a stable object literal without re-creating closures.
 */
export interface ChatSurfaceLayout {
  /** Outer flex gap. Shibboleth uses `gap-4`, embed `gap-2`. */
  outerGap: string
  /** Padding around the transcript scroll area. Shibboleth `pr-4`, embed `px-4`. */
  transcriptScroll: string
  /** Padding around the composer block. Shibboleth `pt-4`, embed `p-4`. */
  inputBlock: string
  /**
   * Breakpoint at which the Aegis panel switches from fixed drawer
   * (below) to in-flow column (at and above). The iframe canvas
   * can't spare 320px below `md`, so embed sets `md` here while
   * the desktop chat page uses `lg`.
   */
  aegisDrawerBreakpoint: "lg" | "md"
  /**
   * When true the conversation-level teacher notes block sticks to
   * the top of the scrolling transcript with a backdrop blur, so
   * students still see it after scrolling through a long thread.
   * Shibboleth turns this on; the embed view leaves it inline.
   */
  stickyConversationNotes: boolean
}

/**
 * Everything the surface can't compute on its own. Generic over the
 * message row type so each adapter can keep its existing wire shape
 * (Shibboleth uses `Message` from `lib/types`; embed uses its own
 * `EmbedMessage` without the token-usage fields).
 */
export interface ChatSurfaceAdapter<M extends ChatBubbleMessage> {
  courseId: string
  conversationId: string | null

  /** Loaded conversation messages, or undefined while React Query / embedGet is in flight. */
  messages: M[] | undefined
  notes: TeacherNote[]
  promptAnalyses: PromptAnalysis[]
  isLoading: boolean

  // Greeting hero fields
  courseName: string | null
  displayName: string | null
  suggestions: string[] | undefined

  // Privacy ack
  needsPrivacyAck: boolean
  onAcknowledgePrivacy: () => Promise<void>

  /**
   * Build the fetch invocation for sending a message. The surface
   * passes it to `useChatStream.send`, which handles the SSE
   * read loop. `analysisAtSend` is the live verdict snapshotted at
   * Send time; the adapter forwards it on the request body so the
   * server can persist it with the new message_id.
   */
  buildSendFetch: (args: {
    content: string
    existingConvId: string | null
    analysisAtSend: PromptAnalysis | null
  }) => () => Promise<Response>

  /**
   * Call /aegis/analyze for the current draft. Shape parity with
   * the hook's `doFetch` parameter; auth wiring is the adapter's
   * problem. `mode` is resolved inside the surface from the stored
   * preference and threaded through so the adapter doesn't need to
   * re-read the mode hook itself.
   */
  fetchLiveAnalysis: (
    content: string,
    previousSuggestions: AegisSuggestion[],
    mode: AegisMode,
    signal: AbortSignal,
  ) => Promise<PromptAnalysis | null>

  /**
   * Call /aegis/rewrite for the draft+suggestion subset. Returns
   * the rewritten text or null on failure / empty body. The banner
   * surfaces failure as a stale-preview clear; no toast. `mode` is
   * passed through for the same reason as `fetchLiveAnalysis`.
   */
  fetchRewrite: (
    draft: string,
    selected: AegisSuggestion[],
    mode: AegisMode,
  ) => Promise<string | null>

  /**
   * Side-effect run after `useChatStream.send` resolves successfully.
   * Shibboleth invalidates React Query caches; embed reloads the
   * conversation detail manually so the optimistic streamed reply
   * is replaced with the persisted row (with metadata).
   *
   * Called with the conversation id the send landed in (the existing
   * one for an append, or the server-assigned one for the first
   * message of a brand-new conv).
   */
  onAfterSend: (landedConvId: string) => Promise<void> | void

  /**
   * Run when the server reports `conversation_created`. Shibboleth
   * navigates to the new url; embed flips its activeConvId state.
   * Distinct from `onAfterSend` because navigation needs to happen
   * after dispatch returns, while `onAfterSend` runs inside the
   * send promise so callers can await query invalidation.
   */
  onConversationCreated: (id: string) => void

  /**
   * Optional per-message renderer; Shibboleth shows feedback
   * thumbs, embed hides them.
   */
  renderFeedbackSlot?: (msg: M) => React.ReactNode

  /**
   * Optional override; defaults to a shared implementation that
   * reads `thinking_transcript` / `tool_events` / `thinking_ms`
   * straight off the message. Both current adapters use the
   * default; left overridable for future callers with different
   * wire shapes.
   */
  getPersistedThinking?: (msg: M) => PersistedThinking | null

  /** Per-conv pinned read-only view; hides the composer entirely. */
  readOnly: boolean
  /** Per-course Aegis feature flag. */
  aegisEnabled: boolean

  /**
   * Translation-namespace-resolved labels.
   */
  labels: ChatSurfaceLabels
  layout: ChatSurfaceLayout
}

/** Default `getPersistedThinking` shared by both current adapters. */
function defaultGetPersistedThinking<
  M extends ChatBubbleMessage & {
    thinking_transcript?: string | null
    tool_events?: Array<{
      name: string
      args?: unknown
      result_summary?: string
      result?: unknown
    }> | null
    thinking_ms?: number | null
    thinking_hidden?: boolean
  },
>(msg: M): PersistedThinking | null {
  return {
    thinking_transcript: msg.thinking_transcript ?? null,
    tool_events: msg.tool_events
      ? msg.tool_events.map((e) => ({
          name: e.name,
          args: e.args,
          resultSummary: e.result_summary,
          result: e.result,
        }))
      : null,
    thinking_ms: msg.thinking_ms ?? null,
    thinking_hidden: msg.thinking_hidden ?? false,
  }
}

export function ChatSurface<M extends ChatBubbleMessage>({
  adapter,
}: {
  adapter: ChatSurfaceAdapter<M>
}) {
  const {
    courseId,
    conversationId,
    messages,
    notes,
    promptAnalyses,
    isLoading,
    courseName,
    displayName,
    suggestions,
    needsPrivacyAck,
    onAcknowledgePrivacy,
    buildSendFetch,
    fetchLiveAnalysis,
    fetchRewrite,
    onAfterSend,
    onConversationCreated,
    renderFeedbackSlot,
    getPersistedThinking,
    readOnly,
    aegisEnabled,
    labels,
    layout,
  } = adapter

  const [input, setInput] = useState("")
  const stream = useChatStream(labels.unknownError)
  const { send, reset } = stream

  // Subject-expertise mode (Beginner/Expert). The panel toggle
  // writes through `useAegisMode`; we only need the value here.
  const [aegisMode] = useAegisMode()

  // Storage-backed; the panel X writes false, the floating pill
  // brings it back. Default true so an aegis-on course shows the
  // panel by default.
  const [panelVisible, setPanelVisible] = useAegisPanelVisible()

  // Live analyzer. Wraps `fetchLiveAnalysis` via the hook so we
  // get debounce + race + accumulator semantics for free. We wrap
  // the adapter's `fetchLiveAnalysis` with the current `aegisMode`
  // so callers don't need to re-read the mode hook themselves; the
  // wrapped closure is memoised against `aegisMode` so a mode flip
  // also invalidates the hook's stored closure (independent of the
  // resetKey, which only handles the cached verdict). Mode is in
  // the resetKey too because the analyzer's draft-match short
  // -circuit would otherwise serve a Beginner verdict to a student
  // who just toggled to Expert (same draft text, cached result).
  const fetchLiveAnalysisWithMode = useCallback(
    (
      content: string,
      previousSuggestions: AegisSuggestion[],
      signal: AbortSignal,
    ) => fetchLiveAnalysis(content, previousSuggestions, aegisMode, signal),
    [fetchLiveAnalysis, aegisMode],
  )
  const liveAnalyzer = useAegisLiveAnalyzer(
    input,
    aegisEnabled,
    fetchLiveAnalysisWithMode,
    `${courseId}:${conversationId ?? "new"}:${aegisMode}`,
  )

  // Reset local state when the conversation changes. `liveAnalyzer`
  // resets its own cache via its resetKey above; we only need to
  // wipe input and clear the SSE buffer here. Adjust-state-on-
  // prop-change during render is the React-docs-sanctioned
  // alternative to setState-in-effect.
  // https://react.dev/learn/you-might-not-need-an-effect#adjusting-some-state-when-a-prop-changes
  const [prevConversationId, setPrevConversationId] = useState(conversationId)
  if (conversationId !== prevConversationId) {
    setPrevConversationId(conversationId)
    reset()
    setInput("")
  }

  // Index notes by message_id for inline display. Notes without a
  // message_id are conversation-level and render in the
  // `renderBeforeMessages` slot below.
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
   * Returns the conversation id this send landed in (the existing
   * one for an append, or the server-assigned one for a brand-new
   * conv signaled via the first SSE event), or null if the send
   * failed before any conv was created.
   */
  const sendMessage = async (
    content: string,
    existingConvId: string | null,
  ): Promise<string | null> => {
    // Snapshot the live analysis on submit; the panel may refresh
    // asynchronously after this point, so we lock in exactly what
    // the student saw when they clicked Send. The adapter ships it
    // alongside the message body so the server can persist it with
    // the new message_id for the History panel.
    const analysisAtSend = liveAnalyzer.consume()
    let landedConvId: string | null = existingConvId
    const ok = await send(
      content,
      buildSendFetch({ content, existingConvId, analysisAtSend }),
      (data) => {
        if (
          data.type === "conversation_created" &&
          typeof data.id === "string"
        ) {
          landedConvId = data.id
        }
      },
    )
    if (ok && landedConvId) {
      await onAfterSend(landedConvId)
    }
    return ok ? landedConvId : null
  }

  // Soft-block intercept. When the student presses Send AND aegis
  // returns non-empty suggestions for the current draft we DON'T
  // dispatch; we set `confirmDraftSend` to the draft string so the
  // next press of Send (with the same content) goes through. The
  // button re-labels to "Send as-is" and the banner stays visible.
  // The analyzer runs on Send (`analyzeNow`) so a fast typer who
  // presses Enter inside the debounce window still gets the chance
  // to see suggestions; `analyzeNow` short-circuits when the cache
  // already matches the input, so no second LLM call for slow
  // typers. Resets when the student edits or after a successful send.
  const [confirmDraftSend, setConfirmDraftSend] = useState<string | null>(null)
  const [submitChecking, setSubmitChecking] = useState(false)

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

    // Second press of Send for the same draft we already soft-blocked
    // on. The student saw the suggestions, decided to send anyway.
    // Dispatch immediately; no second analyzer call (verdict cached).
    if (confirmDraftSend === msg) {
      dispatchSend(msg)
      return
    }

    // Aegis off for the course -> straight send, no checking.
    if (!aegisEnabled) {
      dispatchSend(msg)
      return
    }

    // First Send press with aegis on. Fire (or reuse cached) verdict.
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

  // Banner state ("Aegis has N ideas" tile above the input). Shows
  // whenever the live verdict has suggestions for the current draft
  // AND the student hasn't dismissed it for THIS draft. New input
  // regenerates suggestions and clears the dismissal so the banner
  // can return.
  const [bannerDismissedFor, setBannerDismissedFor] = useState<string | null>(
    null,
  )
  const [rewriting, setRewriting] = useState(false)

  // Reset both `confirmDraftSend` and `bannerDismissedFor` as soon
  // as the draft diverges from the snapshot we last reset against.
  // Adjust-state-on-prop-change during render.
  const [prevDraftInput, setPrevDraftInput] = useState(input)
  if (input !== prevDraftInput) {
    setPrevDraftInput(input)
    if (confirmDraftSend !== null && confirmDraftSend !== input) {
      setConfirmDraftSend(null)
    }
    if (bannerDismissedFor !== null && bannerDismissedFor !== input) {
      setBannerDismissedFor(null)
    }
  }
  const liveSuggestions = liveAnalyzer.analysis?.suggestions ?? []
  const showBanner =
    aegisEnabled &&
    liveSuggestions.length > 0 &&
    bannerDismissedFor !== input

  /**
   * Preview handler. Asks the adapter to call /aegis/rewrite and
   * returns the rewritten draft for the banner to display read-only.
   * Critically does NOT replace the input or dispatch a send; the
   * student decides next by either applying or discarding.
   */
  const handlePreviewIdeas = useCallback(
    async (selected: AegisSuggestion[]): Promise<string | null> => {
      if (rewriting) return null
      const draft = input
      if (!draft.trim() || selected.length === 0) return null
      setRewriting(true)
      try {
        return await fetchRewrite(draft, selected, aegisMode)
      } finally {
        setRewriting(false)
      }
    },
    [fetchRewrite, input, rewriting, aegisMode],
  )

  /**
   * Apply a previewed rewrite to the input. Pre-confirms the soft
   * block for THIS exact text so the next Send press goes straight
   * through; the student already engaged with the suggestions via
   * the preview, so a second soft-block would be busywork. The new
   * analyze run that fires on the rewritten input is still allowed
   * to surface fresh suggestions if it finds any.
   *
   * `liveAnalyzer.reset()` here clears the cached old verdict so
   * the banner doesn't briefly flash the previous draft's
   * suggestions while the new analyze call is in flight (~400ms).
   */
  const handleApplyRewrite = (rewritten: string) => {
    if (!rewritten.trim()) return
    setInput(rewritten)
    setConfirmDraftSend(rewritten)
    liveAnalyzer.reset()
  }

  // Greeting hero in place of the transcript when the route is on
  // a fresh new-chat slot (no conv yet, nothing pending or streaming).
  // The moment a send is dispatched `pendingUserMsg` becomes truthy
  // and the transcript takes over.
  const showGreeting =
    conversationId === null && !stream.streaming && !stream.pendingUserMsg

  const getThinking = getPersistedThinking ?? defaultGetPersistedThinking<M>
  const drawerBp = layout.aegisDrawerBreakpoint
  // Tailwind needs the breakpoint class literal at build time, so
  // we route the two supported values through explicit className
  // strings rather than templating. (Adding a new breakpoint here
  // also needs the new literal in the `outerWrapper` / panel /
  // backdrop lines below.)
  const hideAtBpClass = drawerBp === "lg" ? "lg:hidden" : "md:hidden"
  const stickyAtBpClass =
    drawerBp === "lg"
      ? "lg:static lg:inset-auto lg:z-auto lg:w-80 lg:max-w-none lg:shrink-0 lg:py-0 lg:pr-0 lg:bg-transparent"
      : "md:static md:inset-auto md:z-auto md:w-72 md:max-w-none md:shrink-0 md:py-0 md:pr-0 md:bg-transparent"
  const panelWidthClass = drawerBp === "lg" ? "w-80" : "w-72"

  return (
    <div className={`relative flex flex-1 min-h-0 ${layout.outerGap}`}>
      <div className="flex-1 flex flex-col min-w-0">
        <div className={`flex-1 overflow-y-auto ${layout.transcriptScroll}`}>
          {showGreeting ? (
            <div className="h-full flex items-center justify-center">
              <EmptyChatGreeting
                displayName={displayName}
                courseName={courseName}
                suggestions={suggestions}
                onSuggestionClick={(q) => setInput(q)}
              />
            </div>
          ) : (
            <ChatTranscript<M>
              messages={messages}
              isLoading={isLoading}
              pendingUserMsg={stream.pendingUserMsg}
              streaming={stream.streaming}
              streamedTokens={stream.streamedTokens}
              error={stream.error}
              thinkingTokens={stream.thinkingTokens}
              toolEvents={stream.toolEvents}
              thinkingActive={stream.thinkingActive}
              thinkingDurationMs={stream.thinkingDurationMs}
              thinkingHidden={stream.thinkingHidden}
              bubbleLabels={labels.bubble}
              thinkingLabels={labels.thinking}
              getPersistedThinking={getThinking}
              assistantResponseLabel={labels.assistantResponse}
              renderBeforeMessages={() =>
                conversationNotes.length > 0 ? (
                  // Pin conversation-wide teacher notes to the top
                  // of the scrolling transcript so students still
                  // see them when reading further down a long
                  // conversation. Embed leaves them inline (the
                  // iframe is short enough that they stay visible
                  // anyway, and the sticky backdrop reads oddly
                  // against Moodle's chrome).
                  <div
                    className={
                      layout.stickyConversationNotes
                        ? "sticky top-0 z-10 py-2 bg-background/95 supports-[backdrop-filter]:bg-background/80 backdrop-blur space-y-2"
                        : "space-y-2"
                    }
                  >
                    {conversationNotes.map((note) => (
                      <TeacherNoteInline
                        key={note.id}
                        note={note}
                        label={labels.teacherNote}
                      />
                    ))}
                  </div>
                ) : null
              }
              renderFeedbackSlot={
                !readOnly && renderFeedbackSlot
                  ? (msg) =>
                      msg.role === "assistant" ? renderFeedbackSlot(msg) : null
                  : undefined
              }
              renderAfterMessage={(msg) =>
                notesByMessage.get(msg.id)?.map((note) => (
                  <TeacherNoteInline
                    key={note.id}
                    note={note}
                    label={labels.teacherNote}
                  />
                ))
              }
            />
          )}
        </div>

        {!readOnly && (
          <div className={`${layout.inputBlock} border-t space-y-2`}>
            {needsPrivacyAck && (
              <PrivacyAckBanner onAcknowledge={onAcknowledgePrivacy} />
            )}
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
              // Persistent status row above the input. Three states
              // mirror the right-rail panel so a student with the
              // panel hidden still has a clear "is aegis doing
              // something?" signal. The pending branch covers BOTH
              // the typing-debounce check AND the just-in-time
              // analyzeNow intercept on Send, so the student never
              // wonders "did my Send go through?" while the analyzer
              // is still racing.
              <output
                aria-live="polite"
                className="flex items-center gap-2 rounded-md border bg-muted/40 px-3 py-2 text-xs text-muted-foreground"
              >
                <AegisShieldFilled
                  className={`w-4 h-4 shrink-0 ${liveAnalyzer.pending ? "animate-pulse" : ""}`}
                />
                <span>
                  {liveAnalyzer.pending
                    ? labels.aegisPendingTitle
                    : liveAnalyzer.analysis &&
                        liveAnalyzer.analysis.suggestions.length === 0
                      ? labels.aegisLooksGoodTitle
                      : labels.aegisEmptyTitle}
                </span>
              </output>
            )}
            <form onSubmit={handleSubmit} className="flex gap-2">
              <Input
                value={input}
                onChange={(e) => setInput(e.target.value)}
                placeholder={labels.inputPlaceholder}
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
                  ? labels.aegisChecking
                  : sendNeedsConfirm
                    ? labels.aegisSendAsIs
                    : labels.send}
              </Button>
            </form>
            <p className="text-xs text-muted-foreground text-center">
              {labels.disclaimerBefore}
              <DataHandlingLink label={labels.disclaimerLink} />
              {labels.disclaimerAfter}
            </p>
          </div>
        )}
      </div>
      {aegisEnabled && panelVisible && (
        <>
          {/*
            Below-breakpoint backdrop. The panel renders as a fixed
            drawer at those sizes so the chat column keeps the room
            until the student opens it; the backdrop dismisses on
            tap, mirroring the conversations sidebar's mobile
            behaviour.
          */}
          <div
            className={`${hideAtBpClass} fixed inset-0 z-30 bg-background/60`}
            onClick={() => setPanelVisible(false)}
            aria-hidden="true"
          />
          {/*
            Right-rail Aegis panel. Two layouts driven off the same
            element so visible/dismissed state stays consistent
            across breakpoints. The panel's own X closes both forms.
          */}
          <aside
            className={`fixed inset-y-0 right-0 z-40 ${panelWidthClass} max-w-[90vw] bg-background border-l flex flex-col py-3 pr-3 ${stickyAtBpClass}`}
          >
            <AegisFeedbackPanel
              analyses={promptAnalyses}
              onHide={() => setPanelVisible(false)}
            />
          </aside>
        </>
      )}
      {aegisEnabled && !panelVisible && (
        // "Bring Aegis back" pill. Renders at every breakpoint now
        // that the panel itself adapts (drawer below the breakpoint,
        // in-flow rail at and above); a phone-width iframe student
        // has the same affordance as a desktop one.
        <button
          type="button"
          onClick={() => setPanelVisible(true)}
          className="absolute top-2 right-2 z-20 inline-flex items-center gap-2 rounded-full border bg-background px-3 py-1.5 text-xs font-medium shadow-sm hover:bg-muted/60 transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
          title={labels.aegisShowPanel}
          aria-label={labels.aegisShowPanel}
        >
          <AegisShieldFilled size={16} className="rounded-sm shrink-0" />
          <span>{labels.aegisShowPanelButton}</span>
        </button>
      )}
    </div>
  )
}

/**
 * Renders the privacy-disclosure link. The Shibboleth route uses
 * a TanStack-router `<Link>` so the click stays in-app; the embed
 * route runs inside an iframe and should open the policy in a new
 * tab so the student doesn't lose their conversation. Picked based
 * on whether the surface's surrounding window is the top-level
 * window (Shibboleth) or a framed one (embed).
 *
 * Doing the check in here (rather than via an adapter override)
 * keeps the surface's prop surface small; the heuristic is
 * deterministic and matches what each route already wanted.
 */
function DataHandlingLink({ label }: { label: string }) {
  // `window.top === window` is false inside an iframe (modulo CSP).
  // Guard for SSR / non-window environments by treating "no window"
  // as the in-app case; this component only renders client-side
  // either way.
  const isEmbedded =
    typeof window !== "undefined" && window.top !== window.self
  if (isEmbedded) {
    return (
      <a
        href="/data-handling"
        target="_blank"
        rel="noopener noreferrer"
        className="underline hover:text-foreground"
      >
        {label}
      </a>
    )
  }
  return (
    <Link to="/data-handling" className="underline hover:text-foreground">
      {label}
    </Link>
  )
}
