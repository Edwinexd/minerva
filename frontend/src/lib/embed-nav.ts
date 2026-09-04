import { createContext, useContext } from "react"

/**
 * The embed header lives in `RootLayout`, one level above the embed page that
 * owns the selected-conversation state. This context lifts that selection up so
 * the header's "open in Minerva" link can deep-link to the same chat on the
 * real site instead of always dropping the user on a blank new chat.
 *
 * Null means "no chat selected", which is also the state on first load; the
 * header then points at the course's new-chat route.
 */
export interface EmbedNavValue {
  conversationId: string | null
  setConversationId: (conversationId: string | null) => void
}

export const EmbedNavContext = createContext<EmbedNavValue>({
  conversationId: null,
  setConversationId: () => {},
})

export function useEmbedNav() {
  return useContext(EmbedNavContext)
}
