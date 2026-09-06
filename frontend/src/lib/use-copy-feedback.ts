/**
 * Transient "Copied!" state for copy-to-clipboard buttons.
 *
 * `copy(text, key)` writes through `lib/clipboard.ts` (so the button
 * still works on plain-HTTP LAN URLs where `navigator.clipboard` is
 * undefined) and, only on success, sets `copiedKey` for
 * `COPY_FEEDBACK_MS`. Pages with a single copy button can omit the key
 * and compare against the default.
 *
 * The timer is cleared on unmount and superseded on a second copy, so
 * a component torn down mid-feedback never calls setState after
 * unmount and rapid clicks don't leave an early timer to cancel the
 * later one's feedback.
 */
import { useCallback, useEffect, useRef, useState } from "react"
import { copyToClipboard } from "./clipboard"

/// How long the "Copied!" label stays up.
export const COPY_FEEDBACK_MS = 2000

/// Key used when a call site has only one copy button and passes none.
export const DEFAULT_COPY_KEY = "default"

export function useCopyFeedback(): {
  /// The key most recently copied, or null while no feedback is showing.
  copiedKey: string | null
  copy: (text: string, key?: string) => Promise<void>
} {
  const [copiedKey, setCopiedKey] = useState<string | null>(null)
  const timerRef = useRef<ReturnType<typeof setTimeout> | null>(null)

  useEffect(
    () => () => {
      if (timerRef.current !== null) clearTimeout(timerRef.current)
    },
    [],
  )

  const copy = useCallback(async (text: string, key = DEFAULT_COPY_KEY) => {
    const ok = await copyToClipboard(text)
    if (!ok) return
    if (timerRef.current !== null) clearTimeout(timerRef.current)
    setCopiedKey(key)
    timerRef.current = setTimeout(() => {
      timerRef.current = null
      setCopiedKey(null)
    }, COPY_FEEDBACK_MS)
  }, [])

  return { copiedKey, copy }
}
