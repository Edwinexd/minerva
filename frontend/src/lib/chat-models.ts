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
