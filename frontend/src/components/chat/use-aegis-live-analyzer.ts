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
 * Race handling. We do NOT abort the previous in-flight analyze
 * when a new debounced fire starts ; that was the original design
 * but it broke the accumulator. Aborting causes `fetch` to throw,
 * `.then()` never runs, and the aborted call's suggestions never
 * make it into the coaching-memory accumulator. After ~3 fast
 * iterations the accumulator was empty even though Aegis had
 * coached on several kinds, and the analyzer kept re-raising them.
 *
 * Instead we use a session+generation pair:
 *
 *   * `sessionRef` bumps on genuine end-of-session events
 *     (conversation switch, MIN_LENGTH wipe, Send-driven consume).
 *     In-flight calls whose captured session token doesn't match
 *     the current value drop their result entirely (no merge, no
 *     `setAnalysis`).
 *   * `generationRef` bumps on every fire. The result's `setAnalysis`
 *     is gated on `myGen === generationRef.current` so an older
 *     in-flight call landing after a newer one can't overwrite the
 *     panel.
 *
 * All completed calls within the same session merge their
 * suggestions into the accumulator regardless of generation, so a
 * fast typer who fires three calls during one draft session ends
 * up with all three calls' kinds in the accumulator, not just the
 * latest's. That's the property the server-side already-addressed
 * check needs to actually work.
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
import type { AegisSuggestion, PromptAnalysis } from "@/lib/types"

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
 *
 *                      The hook hands `previousSuggestions` to every
 *                      doFetch call; those are the ACCUMULATED
 *                      suggestions across every iteration of this
 *                      draft session, deduped by `kind` (latest text
 *                      wins per kind). Closures must ship them into
 *                      the request body so the server's
 *                      already-addressed check can see EVERY kind
 *                      the analyzer has coached on so far, not just
 *                      whatever the most recent verdict happened to
 *                      surface.
 *
 *                      Reset (empties accumulated): the input box
 *                      drops below MIN_LENGTH (a delete-back-to-empty
 *                      means the student is starting a fresh draft);
 *                      the conversation switches (resetKey changes);
 *                      or `consume()` ships the verdict with a
 *                      successful Send. Note that `reset()` itself
 *                      does NOT wipe the accumulator ; the rewrite-
 *                      apply path calls reset() to clear the stale
 *                      verdict during the new analyze call's
 *                      ~400ms wait, but the rewrite is still part
 *                      of the same draft session and accumulated
 *                      coaching must survive it.
 * @param resetKey      changes when the conversation context flips
 *                      (e.g. user switched conversations). Bumping
 *                      this clears the cached analysis AND the
 *                      accumulated history so panel content / coaching
 *                      memory from one conversation never leaks into
 *                      another.
 */
export function useAegisLiveAnalyzer(
  input: string,
  aegisEnabled: boolean,
  doFetch: (
    content: string,
    previousSuggestions: AegisSuggestion[],
    signal: AbortSignal,
  ) => Promise<PromptAnalysis | null>,
  resetKey: string | null,
): AegisLiveAnalyzerState {
  const [analysis, setAnalysis] = useState<PromptAnalysis | null>(null)
  const [pending, setPending] = useState(false)
  // Track the last input we actually fired against so an unrelated
  // re-render with the same `input` doesn't refire the analyzer.
  const lastAnalyzed = useRef<string | null>(null)
  // Accumulated coaching memory for the current draft session.
  // Append-only array of every suggestion the analyzer has produced
  // across every iteration of this draft, oldest-first. Shipped to
  // the server on every fire as `previous_suggestions` so the
  // already-addressed check sees the FULL history, not just whatever
  // happened to be in the most recent verdict.
  //
  // We deliberately do NOT dedupe by `kind`. Multiple iterations
  // that all surface `clarity` with different texts ("specify the
  // referent", "name the symbol", "what does 'this' refer to")
  // ARE legitimately different coaching moments and the model
  // benefits from seeing the full sequence ; collapsing them down
  // to "kind: clarity" once would lose the signal that the analyzer
  // has been hammering the same dimension over and over and should
  // stop. We DO dedupe on exact `(kind, text)` match so a sticky
  // LLM that returns identical output two iterations in a row
  // doesn't bloat the bullet list.
  //
  // Why not state: this is read inside a closure that the debounce
  // setTimeout captures. Putting it in useState would either need
  // us to read fresh-state-from-a-ref-anyway, or accept stale
  // captures from a slow render. Using a ref is the simpler path
  // given we never render this list.
  const accumulatedRef = useRef<AegisSuggestion[]>([])
  // Bumps on session-end events (conversation switch, MIN_LENGTH
  // wipe, Send-driven consume). In-flight calls capture this at
  // fire time; if the value has moved by the time the response
  // lands, the call's session is dead and we drop the result.
  const sessionRef = useRef(0)
  // Bumps on every fire (debounce or analyzeNow). Used to gate
  // setAnalysis: only the latest-generation result updates the
  // panel, so an older in-flight call landing after a newer one
  // can't overwrite the displayed verdict.
  const generationRef = useRef(0)
  // Latest in-flight controller. Used by analyzeNow + the
  // session-end paths to abort the network request when we're
  // sure we don't want its result. The debounce path does NOT
  // abort; multiple calls can race to completion and all merge
  // into the accumulator (subject to the session check).
  const abortRef = useRef<AbortController | null>(null)
  // Handle for the queued debounce timeout, so `analyzeNow` can cancel
  // it before it fires. Without this, a setTimeout queued by typing
  // would still fire ~1s later and burn an extra LLM call on the
  // same content analyzeNow is already covering.
  const debounceHandleRef = useRef<ReturnType<typeof setTimeout> | null>(null)
  // True while a Send-driven `analyzeNow` is awaiting its doFetch.
  // The debounce setTimeout checks this and skips when set so a
  // *new* debounce fired by typing AFTER clicking Send doesn't burn
  // a redundant LLM call on the same draft analyzeNow is already
  // resolving.
  const analyzeNowInFlight = useRef(false)

  // Conversation switch (or initial mount with a different
  // resetKey) wipes everything: cached verdict, in-flight request,
  // last-analyzed marker, accumulated coaching memory, AND bumps
  // session so any in-flight call lands on a dead session and is
  // dropped. Without the session bump, a request fired against the
  // previous conversation that happened to land mid-switch would
  // still merge its kinds into the (just-wiped) accumulator and
  // show its verdict on the new conversation's panel.
  //
  // The state resets (`setAnalysis(null)` + `setPending(false)`)
  // happen during render via the adjust-state-on-prop-change
  // pattern; only the genuine side effects (ref mutations, abort,
  // clearTimeout) stay in the effect so we satisfy both
  // react-hooks/set-state-in-effect and react-hooks/refs. See
  // https://react.dev/learn/you-might-not-need-an-effect#adjusting-some-state-when-a-prop-changes
  const [prevResetKey, setPrevResetKey] = useState(resetKey)
  if (resetKey !== prevResetKey) {
    setPrevResetKey(resetKey)
    setAnalysis(null)
    setPending(false)
  }
  useEffect(() => {
    sessionRef.current++
    abortRef.current?.abort()
    abortRef.current = null
    if (debounceHandleRef.current) {
      clearTimeout(debounceHandleRef.current)
      debounceHandleRef.current = null
    }
    analyzeNowInFlight.current = false
    lastAnalyzed.current = null
    accumulatedRef.current = []
  }, [resetKey])

  // "Dead" condition: the analyzer has nothing to show because the
  // course flag is off or the draft is too short. Drop any stale
  // verdict + pending flag eagerly during render so a flag-on-again
  // or quick retype doesn't briefly re-render an old panel before
  // the next debounce tick. Adjust-state-during-render is the
  // React-docs-sanctioned alternative to setState-in-effect; see
  // https://react.dev/learn/you-might-not-need-an-effect#adjusting-some-state-when-a-prop-changes
  const trimmedInput = input.trim()
  const analyzerDormant = !aegisEnabled || trimmedInput.length < MIN_LENGTH
  if (analyzerDormant && analysis !== null) setAnalysis(null)
  if (analyzerDormant && pending) setPending(false)

  useEffect(() => {
    if (!aegisEnabled) return

    const trimmed = input.trim()
    if (trimmed.length < MIN_LENGTH) {
      // Delete-back-to-empty (or anywhere below MIN_LENGTH) is the
      // strongest signal we have that the student is restarting
      // their draft from scratch. Bump session so any in-flight
      // call's result is dropped, abort the network request, and
      // wipe the accumulator so coached kinds from the abandoned
      // draft don't suppress legitimate suggestions on the next one.
      // (Verdict + pending clears live in the during-render block
      // above, since they're pure setState and would otherwise trip
      // the react-hooks/set-state-in-effect rule.)
      sessionRef.current++
      abortRef.current?.abort()
      abortRef.current = null
      accumulatedRef.current = []
      lastAnalyzed.current = null
      return
    }

    if (trimmed === lastAnalyzed.current) return

    debounceHandleRef.current = setTimeout(() => {
      debounceHandleRef.current = null
      // If a Send-driven analyzeNow is currently awaiting, skip;
      // analyzeNow is already covering this draft and the panel
      // will pick up its result. A debounced fire here would just
      // burn a redundant LLM call.
      if (analyzeNowInFlight.current) return
      // Capture session + generation BEFORE firing. The result
      // handler uses these to decide whether to merge into the
      // accumulator (session match) and / or update the displayed
      // analysis (latest generation).
      const mySession = sessionRef.current
      const myGen = ++generationRef.current
      // Each fire gets its own controller. We do NOT abort previous
      // in-flight debounced calls; aborting throws inside fetch and
      // .then never runs, so the aborted call's suggestions never
      // reach the accumulator. The session/generation guards below
      // are what keep stale results from corrupting the panel.
      const controller = new AbortController()
      abortRef.current = controller
      setPending(true)
      // Hand the FULL ACCUMULATED coaching history back to the
      // server, not just the latest verdict. Each iteration's
      // verdict only surfaces 0..=2 current concerns; an iteration
      // ago's `clarity` may have dropped out of the visible verdict
      // because the analyzer moved on to `constraints`, but the
      // student has already been coached on it and we don't want
      // it re-raised. Slice (not direct ref) so a result that
      // arrives during the request can't mutate what was sent.
      const previousSuggestions = accumulatedRef.current.slice()
      doFetch(trimmed, previousSuggestions, controller.signal)
        .then((result) => {
          // Session-end event since we fired? Drop the result
          // entirely (no merge, no display).
          if (sessionRef.current !== mySession) return
          // Only the LATEST generation makes it past this gate.
          // Older in-flight calls landing after a newer one
          // shouldn't overwrite the panel AND shouldn't pollute
          // the accumulator either. Pilot users were explicit:
          // accumulate only what the user actually SAW. A verdict
          // that landed after a newer one already replaced it on
          // the panel was never displayed, so the student wasn't
          // coached on it; suppressing it from the accumulator
          // keeps "previously coached" honest. The trade-off is
          // we drop a few suggestions on a fast typer who fires
          // multiple debounced calls, but those suggestions
          // weren't shown to them anyway, so they don't belong in
          // the already-addressed memory.
          if (myGen !== generationRef.current) return
          if (result) {
            for (const s of result.suggestions) {
              const exists = accumulatedRef.current.some(
                (a) => a.kind === s.kind && a.text === s.text,
              )
              if (!exists) accumulatedRef.current.push(s)
            }
          }
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
          // Only clear pending when this is the latest-generation
          // call AND we're still in the same session; otherwise
          // an older call's finally would clear pending while a
          // newer call is still running.
          if (
            sessionRef.current === mySession &&
            myGen === generationRef.current
          ) {
            setPending(false)
          }
        })
    }, DEBOUNCE_MS)

    return () => {
      if (debounceHandleRef.current) {
        clearTimeout(debounceHandleRef.current)
        debounceHandleRef.current = null
      }
    }
  }, [input, aegisEnabled, doFetch])

  // Stable callbacks. Both consumers wipe local state but only
  // `reset` also nulls `lastAnalyzed`; `consume` retains it so
  // the same analysis isn't re-fetched on the next keystroke
  // when the input string hasn't actually changed yet.
  //
  // `reset()` deliberately does NOT wipe the accumulator OR bump
  // session: it's called by the rewrite-apply path (chat-page +
  // embed-page) to clear the stale verdict during the ~400ms wait
  // for the rewritten input's own analyze call, but the rewrite is
  // just AI-assisted iteration on the SAME draft session ; coaching
  // memory must survive. The accumulator gets wiped on the genuine
  // end-of-session events instead: `consume()` (Send went through),
  // the `resetKey` effect (conversation switch), and the MIN_LENGTH
  // branch in the input effect (delete-back-to-empty signals a
  // fresh draft).
  const reset = () => {
    abortRef.current?.abort()
    abortRef.current = null
    lastAnalyzed.current = null
    setAnalysis(null)
    setPending(false)
  }
  const consume = (): PromptAnalysis | null => {
    const v = analysis
    sessionRef.current++
    accumulatedRef.current = []
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
    // on the queue would fire ~1s later and burn a redundant LLM
    // call on the same content analyzeNow is already covering.
    if (debounceHandleRef.current) {
      clearTimeout(debounceHandleRef.current)
      debounceHandleRef.current = null
    }
    // Capture session + generation, then fire. Same race-handling
    // shape as the debounce path: the accumulator merge is gated on
    // session match, the panel update is gated on latest generation.
    const mySession = sessionRef.current
    const myGen = ++generationRef.current
    const controller = new AbortController()
    abortRef.current = controller
    analyzeNowInFlight.current = true
    setPending(true)
    const previousSuggestions = accumulatedRef.current.slice()
    try {
      const result = await doFetch(
        trimmed,
        previousSuggestions,
        controller.signal,
      )
      if (sessionRef.current !== mySession) return null
      // Only accumulate + display when this is still the latest
      // generation; an older analyzeNow that lost a race shouldn't
      // pollute coaching memory the student never saw. Same rule
      // as the debounce path.
      if (myGen !== generationRef.current) return result
      if (result) {
        for (const s of result.suggestions) {
          const exists = accumulatedRef.current.some(
            (a) => a.kind === s.kind && a.text === s.text,
          )
          if (!exists) accumulatedRef.current.push(s)
        }
      }
      lastAnalyzed.current = trimmed
      setAnalysis(result)
      return result
    } catch (e) {
      if (controller.signal.aborted) return null
      console.warn("aegis live analyzer (immediate):", e)
      return null
    } finally {
      analyzeNowInFlight.current = false
      if (
        sessionRef.current === mySession &&
        myGen === generationRef.current
      ) {
        setPending(false)
      }
    }
  }

  return { analysis, pending, reset, consume, analyzeNow }
}
