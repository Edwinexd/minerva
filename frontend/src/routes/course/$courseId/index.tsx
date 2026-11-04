import { createFileRoute } from "@tanstack/react-router"
import { CourseIndex } from "@/components/course/course-index"
import { WORKSPACE_TABS, type WorkspaceTab } from "@/components/credit-transfer/tabs"

export const Route = createFileRoute("/course/$courseId/")({
  component: () => <CourseIndex useParams={Route.useParams} useSearch={Route.useSearch} />,
  // Tab of a credit transfer workspace page; courses ignore it.
  validateSearch: (s: Record<string, unknown>): { tab?: WorkspaceTab } => ({
    tab: (WORKSPACE_TABS as readonly unknown[]).includes(s.tab)
      ? (s.tab as WorkspaceTab)
      : undefined,
  }),
})
