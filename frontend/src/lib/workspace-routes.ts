/**
 * Where a workspace's conversations live
 * (`/workspaces/{id}/conversations/{conversationId}`), the same for every
 * kind. `null` is a new one. Credit transfer chats used to live under
 * `/cases/`, which now redirects here.
 */
export function chatLink(workspaceId: string, conversationId: string | null) {
  return conversationId === null
    ? ({ to: "/workspaces/$workspaceId/conversations/new", params: { workspaceId } } as const)
    : ({
        to: "/workspaces/$workspaceId/conversations/$conversationId",
        params: { workspaceId, conversationId },
      } as const)
}
