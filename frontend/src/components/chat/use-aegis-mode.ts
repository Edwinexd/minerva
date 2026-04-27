/**
 * Shared `AegisMode` state hook.
 *
 * Owns the single source of truth for the student's self-declared
 * subject expertise; consumed by the panel (renders the toggle
 * badge) AND by the parent chat pages (ships the value as `mode`
 * on every `/aegis/analyze` request so the server-side rubric
 * calibrates accordingly).
 *
 * Persisted in localStorage so the choice survives reloads. Cross-
 * tab synced via the `storage` event so flipping the toggle in one
 * tab updates open chat tabs in the same browser without a reload.
 *
 * The hook intentionally keeps no per-component state: every caller
 * gets the same value, every setter writes through to storage, and
 * mounted listeners pick up writes from sibling tabs. Same hook
 * call signature in both consumers (chat-page, embed-page) so the
 * value is mounted exactly once per UI tree even when both the
 * panel and the parent want to read it.
 */
import { useEffect, useState } from "react"

export type AegisMode = "beginner" | "expert"

const STORAGE_KEY = "minerva.aegis.mode"
const DEFAULT_MODE: AegisMode = "beginner"

function readStored(): AegisMode {
  if (typeof window === "undefined") return DEFAULT_MODE
  try {
    const raw = window.localStorage.getItem(STORAGE_KEY)
    if (raw === "beginner" || raw === "expert") return raw
  } catch {
    // Storage unavailable (private mode, quota, etc); fall through.
  }
  return DEFAULT_MODE
}

function writeStored(mode: AegisMode): void {
  if (typeof window === "undefined") return
  try {
    window.localStorage.setItem(STORAGE_KEY, mode)
  } catch {
    // ignore; the hook still works in-memory for this tab.
  }
}

export function useAegisMode(): [AegisMode, (m: AegisMode) => void] {
  // Synchronous initial read off localStorage so the panel doesn't
  // flash the default mode for one frame before settling on the
  // persisted value.
  const [mode, setModeState] = useState<AegisMode>(() => readStored())

  // Cross-tab sync. The storage event only fires for OTHER tabs
  // (not the writer), so this can never feed back into itself.
  useEffect(() => {
    const onStorage = (e: StorageEvent) => {
      if (e.key !== STORAGE_KEY) return
      if (e.newValue === "beginner" || e.newValue === "expert") {
        setModeState(e.newValue)
      }
    }
    window.addEventListener("storage", onStorage)
    return () => window.removeEventListener("storage", onStorage)
  }, [])

  const setMode = (next: AegisMode) => {
    setModeState(next)
    writeStored(next)
  }
  return [mode, setMode]
}
