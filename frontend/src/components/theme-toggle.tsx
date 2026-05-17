/**
 * Single-button theme toggle. Cycles light -> dark -> system,
 * showing the icon for the *currently chosen* mode (NOT the
 * resolved one) so the user can tell at a glance whether they're
 * on explicit Light/Dark or following the OS.
 *
 * Lives in the page header next to the language switcher. Reads /
 * writes via `useTheme`, which owns the class flip on `<html>` and
 * the localStorage persistence.
 */
import { Monitor, Moon, Sun } from "lucide-react"
import { useTranslation } from "react-i18next"

import { useTheme, type Theme } from "@/lib/use-theme"

const ICONS: Record<Theme, typeof Sun> = {
  light: Sun,
  dark: Moon,
  system: Monitor,
}

export function ThemeToggle() {
  const { t } = useTranslation()
  const { theme, cycle } = useTheme()
  const Icon = ICONS[theme]

  // aria-label reflects the action ("Switch to dark mode") rather
  // than the current state, since screen-reader users care about
  // what pressing the button will do.
  const nextTheme: Theme =
    theme === "light" ? "dark" : theme === "dark" ? "system" : "light"
  const label = t("theme.switchTo", {
    next: t(`theme.modes.${nextTheme}`),
  })
  // Tooltip text describes the current state (cycle is the action).
  const tooltip = t("theme.current", {
    mode: t(`theme.modes.${theme}`),
  })

  return (
    <button
      type="button"
      onClick={cycle}
      aria-label={label}
      title={tooltip}
      className="inline-flex items-center justify-center rounded-md border bg-background w-8 h-8 text-muted-foreground hover:text-foreground hover:bg-muted transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring/50"
    >
      <Icon className="w-4 h-4" aria-hidden="true" />
    </button>
  )
}
