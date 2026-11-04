export const WORKSPACE_TABS = ["cases", "plans", "assistant"] as const
export type WorkspaceTab = (typeof WORKSPACE_TABS)[number]
