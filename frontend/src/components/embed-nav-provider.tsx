import { useMemo, useState, type ReactNode } from "react"
import { EmbedNavContext } from "@/lib/embed-nav"

/**
 * Holds the embed page's selected conversation for the header above it.
 * Separate from `@/lib/embed-nav` so each file exports only components or only
 * hooks, which is what react-refresh needs to hot-reload either one.
 */
export function EmbedNavProvider({ children }: { children: ReactNode }) {
  const [conversationId, setConversationId] = useState<string | null>(null)
  const value = useMemo(
    () => ({ conversationId, setConversationId }),
    [conversationId],
  )
  return <EmbedNavContext.Provider value={value}>{children}</EmbedNavContext.Provider>
}
