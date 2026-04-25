/**
 * Shared sidebar conversation list used by both the regular Shibboleth
 * chat page and the LTI embed iframe.
 *
 * Renders, in order:
 *   1. The user's own conversations (with a `*` marker on user-pinned rows).
 *   2. A "Pinned by teacher" section, filtered to pins the user doesn't
 *      already own a copy of, with attribution above each title.
 *
 * Rows are rendered through `renderRow`, so each caller decides whether
 * the row is a `<Link>` (regular page, drives the router) or a
 * `<button>` (embed page, just flips local state). Loading skeletons
 * are handled here so neither caller has to repeat them.
 */
import React from "react"

import { Skeleton } from "@/components/ui/skeleton"

export interface SidebarConversation {
  id: string
  title: string | null
  /** True if the user themselves pinned this conversation (separate from teacher-pinned). */
  pinned?: boolean
}

export interface SidebarPinnedConversation {
  id: string
  title: string | null
  user_eppn: string | null
  user_display_name: string | null
}

export interface ConversationListLabels {
  pinned: string
  newConversation: string
  conversation: string
  pinnedByTeacher: string
  studentFallback: string
}

/**
 * Caller-supplied row renderer. Lets each page choose `<Link>` (regular
 * chat, hooks into tanstack-router) or `<button>` (embed, flips local
 * state) without this component knowing about routing.
 */
export type ConversationRowRenderer = (props: {
  conversationId: string
  className: string
  children: React.ReactNode
}) => React.ReactNode

export function ConversationList({
  conversations,
  conversationsLoading = false,
  pinned,
  pinnedLoading = false,
  activeConversationId,
  renderRow,
  labels,
}: {
  conversations: SidebarConversation[] | undefined
  conversationsLoading?: boolean
  pinned?: SidebarPinnedConversation[]
  pinnedLoading?: boolean
  activeConversationId: string | null
  renderRow: ConversationRowRenderer
  labels: ConversationListLabels
}) {
  // Don't list a teacher-pinned chat twice if the viewer is also its
  // owner -- their copy already appears in the top section.
  const sidebarPinned = (pinned ?? []).filter(
    (p) => !conversations?.some((c) => c.id === p.id),
  )

  const rowClass = (id: string) =>
    `block w-full text-left px-3 py-2 rounded text-sm truncate ${
      activeConversationId === id
        ? "bg-secondary text-secondary-foreground"
        : "hover:bg-muted"
    }`

  return (
    <>
      {conversationsLoading &&
        Array.from({ length: 3 }).map((_, i) => (
          <Skeleton key={i} className="h-9 w-full mb-1" />
        ))}
      {conversations?.map((conv) =>
        renderRow({
          conversationId: conv.id,
          className: rowClass(conv.id),
          children: (
            <>
              {conv.pinned && (
                <span className="mr-1" title={labels.pinned}>
                  *
                </span>
              )}
              {conv.title || labels.newConversation}
            </>
          ),
        }),
      )}
      {sidebarPinned.length > 0 && (
        <>
          <div className="text-xs font-medium text-muted-foreground pt-3 pb-1 border-t mt-2">
            {labels.pinnedByTeacher}
          </div>
          {pinnedLoading && <Skeleton className="h-9 w-full mb-1" />}
          {sidebarPinned.map((conv) =>
            renderRow({
              conversationId: conv.id,
              className: rowClass(conv.id),
              children: (
                <>
                  <span className="text-muted-foreground text-xs">
                    {conv.user_display_name ||
                      conv.user_eppn ||
                      labels.studentFallback}
                  </span>
                  <span className="block">{conv.title || labels.conversation}</span>
                </>
              ),
            }),
          )}
        </>
      )}
    </>
  )
}
