/**
 * Per-user "is the right-rail Feedback panel currently expanded?"
 * preference.
 *
 * Lives next to `useAegisMode`: storage-backed so the choice rides
 * across sessions, cross-tab synced via the `storage` event so
 * collapsing the panel in one chat tab also collapses it in the
 * other open tabs (and the embed iframe, since it shares the same
 * origin's localStorage).
 *
 * The course-level `aegisEnabled` flag still gates whether the panel
 * exists at all -- this hook only controls visibility WHEN the
 * feature is on. A student who finds the panel distracting can
 * dismiss it; a "Show suggestions" affordance in the chat brings
 * it back when wanted.
 *
 * Default is visible: a student opting into a course with aegis on
 * should see the panel by default (otherwise the feature is invisible
 * and never gets discovered).
 */
import { useEffect, useState } from "react"

const STORAGE_KEY = "minerva.aegis.panel.visible"
const DEFAULT_VISIBLE = true

function readStored(): boolean {
  if (typeof window === "undefined") return DEFAULT_VISIBLE
  try {
    const raw = window.localStorage.getItem(STORAGE_KEY)
    if (raw === "true") return true
    if (raw === "false") return false
  } catch {
    // Storage unavailable (private mode, quota, etc) -- fall through.
  }
  return DEFAULT_VISIBLE
}

function writeStored(visible: boolean): void {
  if (typeof window === "undefined") return
  try {
    window.localStorage.setItem(STORAGE_KEY, visible ? "true" : "false")
  } catch {
    // ignore -- the hook still works in-memory for this tab.
  }
}

export function useAegisPanelVisible(): [boolean, (v: boolean) => void] {
  const [visible, setVisibleState] = useState<boolean>(() => readStored())

  // Cross-tab sync. The `storage` event only fires for OTHER tabs
  // (not the writer), so this can never feed back into itself.
  useEffect(() => {
    const onStorage = (e: StorageEvent) => {
      if (e.key !== STORAGE_KEY) return
      if (e.newValue === "true") setVisibleState(true)
      else if (e.newValue === "false") setVisibleState(false)
    }
    window.addEventListener("storage", onStorage)
    return () => window.removeEventListener("storage", onStorage)
  }, [])

  const setVisible = (next: boolean) => {
    setVisibleState(next)
    writeStored(next)
  }
  return [visible, setVisible]
}
