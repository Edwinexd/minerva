export const WORKSPACE_TABS = ["cases", "plans", "rules", "assistant"] as const
export type WorkspaceTab = (typeof WORKSPACE_TABS)[number]
