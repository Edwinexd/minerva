/**
 * Live aegis analyzer hook.
 *
 * Watches a chat input string and (when aegis is enabled for the
 * course) calls `POST /aegis/analyze` on a debounce so the right-rail
 * Feedback panel updates while the student is typing; BEFORE they
 * press Send. The verdict the student ultimately accepts is what
 * gets persisted; cached locally so the chat-page can ship it
 * alongside the message body.
 *
 * Auth is the caller's problem: the Shibboleth route runs on a
 * cookie + dev-user header, the embed route ships its token in the
 * request body, and this hook stays neutral by taking a `doFetch`
 * closure that wires whichever flow applies.
 *
 * Cancellation: each new debounced fire aborts the previous in-flight
 * request via AbortController, so a fast typer never sees a stale
 * verdict win the race.
 *
 * Cost shaping:
 *   * `DEBOUNCE_MS`; pause length before we hit the analyzer.
 *     Too short and a steady typer burns LLM calls per word; too
 *     long and the panel feels stuck. 400ms is short enough that
 *     a student who finishes their thought sees feedback almost
 *     immediately, but long enough to coalesce intra-word pauses
 *     so we don't fire mid-sentence.
 *   * `MIN_LENGTH`; inputs shorter than this are too tiny to score
 *     meaningfully. The analyzer would just say "missing constraints"
 *     for every two-word query, which is noise.
 *   * Skip when content is unchanged from the last analyzed value;
 *     a render that doesn't actually edit the input shouldn't refire.
 */
import { useEffect, useRef, useState } from "react"
import type { PromptAnalysis } from "@/lib/types"

const DEBOUNCE_MS = 400
const MIN_LENGTH = 8

export interface AegisLiveAnalyzerState {
  /** Latest verdict from the analyzer, or null if none yet / cleared. */
  analysis: PromptAnalysis | null
  /** True while a request is in flight. Drives the panel's pending state. */
  pending: boolean
  /** Manually drop the cached analysis (e.g. after a successful send). */
  reset: () => void
  /** Hand back the current verdict so the caller can ship it with send(). */
  consume: () => PromptAnalysis | null
  /**
   * Force an immediate analyze call against the current input value,
   * cancelling any pending debounce or in-flight call. Used by the
   * Send-button handler so the just-in-time intercept reflects the
   * student's actual final draft, not whatever the debounced cache
   * happens to hold (which may be stale or null when they typed and
   * sent within the debounce window). Returns the verdict (or null
   * on soft-fail / aegis-off / empty input). Updates `analysis` and
   * `pending` in lockstep so the panel reflects the call.
   */
  analyzeNow: (content: string) => Promise<PromptAnalysis | null>
}

/**
 * @param input         current value of the chat input box
 * @param aegisEnabled  course-level feature flag; when false the hook
 *                      no-ops and `analysis` stays null
 * @param doFetch       caller-supplied analyze call. Throws on
 *                      transport failure; returns null when the
 *                      server soft-failed (aegis disabled, analyzer
 *                      hiccup). Hook treats null as "no panel
 *                      content for this turn".
 * @param resetKey      changes when the conversation context flips
 *                      (e.g. user switched conversations). Bumping
 *                      this clears the cached analysis so panel
 *                      content from one conversation never leaks
 *                      into another.
 */
export function useAegisLiveAnalyzer(
  input: string,
  aegisEnabled: boolean,
  doFetch: (
    content: string,
    signal: AbortSignal,
  ) => Promise<PromptAnalysis | null>,
  resetKey: string | null,
): AegisLiveAnalyzerState {
  const [analysis, setAnalysis] = useState<PromptAnalysis | null>(null)
  const [pending, setPending] = useState(false)
  // Track the last input we actually fired against so an unrelated
  // re-render with the same `input` doesn't refire the analyzer.
  const lastAnalyzed = useRef<string | null>(null)
  const abortRef = useRef<AbortController | null>(null)
  // Handle for the queued debounce timeout, so `analyzeNow` can cancel
  // it before it fires. Without this, a setTimeout queued by typing
  // would still fire ~1s later and abort an in-flight `analyzeNow`
  // call (its first action is `abortRef.current?.abort()`), causing
  // the just-in-time intercept on Send to return null and the message
  // to slip through ungated.
  const debounceHandleRef = useRef<ReturnType<typeof setTimeout> | null>(null)
  // True while a Send-driven `analyzeNow` is awaiting its doFetch.
  // The debounce setTimeout checks this and skips when set, so a
  // *new* debounce fired by typing AFTER clicking Send (which the
  // setTimeout-handle clear above can't reach) still can't abort
  // the in-flight analyzeNow controller.
  const analyzeNowInFlight = useRef(false)

  // Conversation switch (or initial mount with a different
  // resetKey) wipes everything: cached verdict, in-flight request,
  // last-analyzed marker. Without this, switching from one chat to
  // another would briefly show the previous chat's panel content.
  useEffect(() => {
    abortRef.current?.abort()
    abortRef.current = null
    if (debounceHandleRef.current) {
      clearTimeout(debounceHandleRef.current)
      debounceHandleRef.current = null
    }
    analyzeNowInFlight.current = false
    lastAnalyzed.current = null
    setAnalysis(null)
    setPending(false)
  }, [resetKey])

  useEffect(() => {
    if (!aegisEnabled) {
      // Flag flipped off mid-session: drop any stale verdict so
      // the panel hides cleanly. The actual hide happens in the
      // chat layout via the same flag, but clearing here keeps
      // a flag-on-again path from briefly re-rendering an old
      // verdict before the next debounce tick.
      if (analysis !== null) setAnalysis(null)
      return
    }

    const trimmed = input.trim()
    if (trimmed.length < MIN_LENGTH) {
      // Cancel any in-flight call from an earlier longer input
      // so a delete-back-to-empty doesn't briefly show a stale
      // verdict landing after the user wiped the box.
      abortRef.current?.abort()
      abortRef.current = null
      if (analysis !== null) setAnalysis(null)
      lastAnalyzed.current = null
      return
    }

    if (trimmed === lastAnalyzed.current) return

    debounceHandleRef.current = setTimeout(() => {
      debounceHandleRef.current = null
      // If a Send-driven analyzeNow is currently awaiting, leave it
      // alone; aborting its controller here is exactly the race
      // that lets ungated messages through. The user pressed Send,
      // analyzeNow is racing to deliver a verdict; the panel will
      // pick up its result via setAnalysis.
      if (analyzeNowInFlight.current) return
      // Fresh AbortController per fire; cancels whatever's still
      // in flight from the previous debounce tick.
      abortRef.current?.abort()
      const controller = new AbortController()
      abortRef.current = controller
      setPending(true)
      doFetch(trimmed, controller.signal)
        .then((result) => {
          if (controller.signal.aborted) return
          lastAnalyzed.current = trimmed
          setAnalysis(result)
        })
        .catch((e) => {
          if (controller.signal.aborted) return
          // Network errors are non-fatal; the panel just shows
          // the previous verdict (or empty state). We don't surface
          // them since the analyzer is advisory; the user's send
          // path is unaffected.
          console.warn("aegis live analyzer:", e)
        })
        .finally(() => {
          if (!controller.signal.aborted) setPending(false)
        })
    }, DEBOUNCE_MS)

    return () => {
      if (debounceHandleRef.current) {
        clearTimeout(debounceHandleRef.current)
        debounceHandleRef.current = null
      }
    }
    // `analysis` deliberately omitted; the effect's job is to
    // react to INPUT changes, not to its own setAnalysis writes.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [input, aegisEnabled, doFetch])

  // Stable callbacks. Both consumers wipe local state but only
  // `reset` also nulls `lastAnalyzed`; `consume` retains it so
  // the same analysis isn't re-fetched on the next keystroke
  // when the input string hasn't actually changed yet.
  const reset = () => {
    abortRef.current?.abort()
    abortRef.current = null
    lastAnalyzed.current = null
    setAnalysis(null)
  }
  const consume = (): PromptAnalysis | null => {
    const v = analysis
    setAnalysis(null)
    return v
  }

  const analyzeNow = async (
    content: string,
  ): Promise<PromptAnalysis | null> => {
    if (!aegisEnabled) return null
    const trimmed = content.trim()
    if (trimmed.length < MIN_LENGTH) return null
    // If the cache already matches this exact content AND we're
    // not currently mid-flight, short-circuit; no point burning
    // a second LLM call on the same draft just because the user
    // pressed Send a second after the debounce already settled.
    if (!pending && trimmed === lastAnalyzed.current && analysis !== null) {
      return analysis
    }
    // Cancel any queued debounce timeout BEFORE we install our own
    // controller. Without this, a setTimeout the user's typing put
    // on the queue would fire ~1s later, call abortRef.current?.abort()
    // (which is now OUR controller), and short-circuit the doFetch
    // below to a null verdict; exactly the race that lets a Send
    // through ungated when feedback isn't ready yet.
    if (debounceHandleRef.current) {
      clearTimeout(debounceHandleRef.current)
      debounceHandleRef.current = null
    }
    // Otherwise fire fresh; abort any in-flight or pending call
    // first so this one wins the race.
    abortRef.current?.abort()
    const controller = new AbortController()
    abortRef.current = controller
    analyzeNowInFlight.current = true
    setPending(true)
    try {
      const result = await doFetch(trimmed, controller.signal)
      if (controller.signal.aborted) return null
      lastAnalyzed.current = trimmed
      setAnalysis(result)
      return result
    } catch (e) {
      if (controller.signal.aborted) return null
      console.warn("aegis live analyzer (immediate):", e)
      return null
    } finally {
      analyzeNowInFlight.current = false
      if (!controller.signal.aborted) setPending(false)
    }
  }

  return { analysis, pending, reset, consume, analyzeNow }
}
