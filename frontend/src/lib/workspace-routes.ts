import type { WorkspaceKind } from "./types"

/**
 * Where a workspace's conversations live: a credit transfer workspace
 * calls them cases (`/workspaces/{id}/cases/{caseId}`), every other kind
 * conversations (`/workspaces/{id}/conversations/{conversationId}`).
 * `null` is a new one.
 */
export function chatLink(
  kind: WorkspaceKind | undefined,
  workspaceId: string,
  conversationId: string | null,
) {
  if (kind === "credit_transfer") {
    return conversationId === null
      ? ({ to: "/workspaces/$workspaceId/cases/new", params: { workspaceId } } as const)
      : ({
          to: "/workspaces/$workspaceId/cases/$caseId",
          params: { workspaceId, caseId: conversationId },
        } as const)
  }
  return conversationId === null
    ? ({ to: "/workspaces/$workspaceId/conversations/new", params: { workspaceId } } as const)
    : ({
        to: "/workspaces/$workspaceId/conversations/$conversationId",
        params: { workspaceId, conversationId },
      } as const)
}
