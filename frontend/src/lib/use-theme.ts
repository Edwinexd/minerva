/**
 * Three-state theme hook (light / dark / system).
 *
 *   * "light" / "dark"  ; explicit preference, persisted to localStorage,
 *                         survives across sessions.
 *   * "system"          ; follow `prefers-color-scheme`; re-resolves
 *                         live when the OS theme flips.
 *
 * The actual class flip lives on `<html>` so Tailwind v4's
 * `@custom-variant dark (&:is(.dark *))` picks it up everywhere.
 *
 * An inline `<script>` in `index.html` runs the same resolution
 * BEFORE first paint to avoid the light-then-dark flash; the storage
 * key + mode strings here must stay in sync with that script.
 */
import { useCallback, useEffect, useState } from "react"

export const THEME_STORAGE_KEY = "minerva-theme"

export type Theme = "light" | "dark" | "system"

/** Reads the stored preference, treating anything unrecognised as "system". */
function readStoredTheme(): Theme {
  try {
    const raw = localStorage.getItem(THEME_STORAGE_KEY)
    if (raw === "light" || raw === "dark" || raw === "system") {
      return raw
    }
  } catch {
    // localStorage unavailable (private mode, etc).
  }
  return "system"
}

function applyTheme(resolved: "light" | "dark") {
  const root = document.documentElement
  if (resolved === "dark") root.classList.add("dark")
  else root.classList.remove("dark")
}

function readSystemPrefersDark(): boolean {
  if (typeof window === "undefined" || !window.matchMedia) return false
  return window.matchMedia("(prefers-color-scheme: dark)").matches
}

export function useTheme(): {
  theme: Theme
  resolved: "light" | "dark"
  setTheme: (next: Theme) => void
  /** Cycle through light -> dark -> system -> light. Convenience for a single button toggle. */
  cycle: () => void
} {
  const [theme, setThemeState] = useState<Theme>(() => readStoredTheme())
  // Tracked as state (rather than recomputed each render) only so the
  // OS-theme-change listener below has somewhere to push updates. With
  // `theme = "system"` and the OS theme flipping, this is what causes
  // the re-render that flips `resolved`. For explicit modes the value
  // is ignored.
  const [systemPrefersDark, setSystemPrefersDark] = useState<boolean>(() =>
    readSystemPrefersDark(),
  )

  // `resolved` is derived, not stored: that way nothing can drift out
  // of sync with `theme`, and the lint rule that bans setState-in-effect
  // (which the previous shape tripped) doesn't apply.
  const resolved: "light" | "dark" =
    theme === "light"
      ? "light"
      : theme === "dark"
        ? "dark"
        : systemPrefersDark
          ? "dark"
          : "light"

  // Apply the class + persist whenever the resolution changes. Storage
  // writes are wrapped because localStorage can throw in private-mode
  // Safari and a couple of WebViews.
  useEffect(() => {
    applyTheme(resolved)
    try {
      localStorage.setItem(THEME_STORAGE_KEY, theme)
    } catch {
      // Best-effort persistence; the in-memory state still works.
    }
  }, [theme, resolved])

  // While in "system" mode, follow OS-level theme flips without a reload.
  // No-op for explicit modes (the listener still mounts, but flipping
  // the OS preference there only updates `systemPrefersDark`, which
  // doesn't feed `resolved`).
  useEffect(() => {
    if (typeof window === "undefined" || !window.matchMedia) return
    const mql = window.matchMedia("(prefers-color-scheme: dark)")
    const onChange = () => setSystemPrefersDark(mql.matches)
    // Some older Safaris ship `addListener`/`removeListener` only.
    if (typeof mql.addEventListener === "function") {
      mql.addEventListener("change", onChange)
      return () => mql.removeEventListener("change", onChange)
    }
    mql.addListener(onChange)
    return () => mql.removeListener(onChange)
  }, [])

  const setTheme = useCallback((next: Theme) => {
    setThemeState(next)
  }, [])

  const cycle = useCallback(() => {
    setThemeState((prev) =>
      prev === "light" ? "dark" : prev === "dark" ? "system" : "light",
    )
  }, [])

  return { theme, resolved, setTheme, cycle }
}
