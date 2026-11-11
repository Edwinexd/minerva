/**
 * Optional friendly-name overrides per known chat-model id. The server
 * already supplies a `display_name`; this map is just a place to refine
 * presentation client-side without a redeploy of the backend catalog.
 * Falls back to the server `display_name` for anything not listed.
 */
export const CHAT_MODEL_DISPLAY: Record<string, { name: string }> = {
  "gpt-oss-120b": { name: "GPT-OSS 120B (Cerebras)" },
  "gpt-4o-mini": { name: "GPT-4o mini" },
  "gpt-4o": { name: "GPT-4o" },
}

export function chatModelDisplayName(m: {
  model: string
  display_name: string
}): string {
  return CHAT_MODEL_DISPLAY[m.model]?.name ?? m.display_name
}

/** Milliseconds as seconds with one decimal, e.g. `0.8`. */
export function formatSeconds(ms: number, locale?: string): string {
  return (ms / 1000).toLocaleString(locale, {
    minimumFractionDigits: 1,
    maximumFractionDigits: 1,
  })
}

/** A token rate rounded to a whole number, e.g. `1,200`. */
export function formatRate(perSecond: number, locale?: string): string {
  return Math.round(perSecond).toLocaleString(locale)
}
