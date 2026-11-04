import type { Verdict } from "@/lib/types"

export type Tone = "ok" | "warn" | "bad" | "info" | "neutral"

/** Background, border and text per tone, each pair at AA contrast. */
export const TONE_CLASS: Record<Tone, string> = {
  ok: "border-green-300 bg-green-50 text-green-800 dark:border-green-800 dark:bg-green-950/50 dark:text-green-200",
  warn: "border-amber-300 bg-amber-50 text-amber-900 dark:border-amber-800 dark:bg-amber-950/50 dark:text-amber-200",
  bad: "border-red-300 bg-red-50 text-red-800 dark:border-red-800 dark:bg-red-950/50 dark:text-red-200",
  info: "border-blue-300 bg-blue-50 text-blue-800 dark:border-blue-800 dark:bg-blue-950/50 dark:text-blue-200",
  neutral: "border-border bg-muted text-muted-foreground",
}

export const VERDICT_TONE: Record<Verdict, Tone> = { full: "ok", partial: "warn", none: "bad" }
