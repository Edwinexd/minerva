import { useNavigate } from "@tanstack/react-router"
import { useQuery } from "@tanstack/react-query"
import { courseQuery } from "@/lib/queries"
import { Skeleton } from "@/components/ui/skeleton"
import { chatLink } from "@/lib/workspace-routes"
import { CreditTransferWorkspacePage } from "@/components/credit-transfer/workspace-page"
import type { WorkspaceTab } from "@/components/credit-transfer/tabs"

export function CourseIndex({
  courseId,
  useSearch,
}: {
  courseId: string
  useSearch: () => { tab?: WorkspaceTab }
}) {
  const { tab } = useSearch()
  const navigate = useNavigate()
  // Course load gates the redirect to a fresh `/new` chat. Previously
  // this auto-selected the most recent conversation, which polluted
  // the context window any time a student (or LTI launch) reopened the
  // course. The history sidebar is still one click away on the chat
  // page itself.
  const { data: course, isLoading: courseLoading } = useQuery(courseQuery(courseId))

  // A credit transfer workspace has a page of its own: its chats, the
  // DSV plans, SU's rules and assistant access, rather than a chat to
  // land in.
  if (course?.kind === "credit_transfer") {
    return <CreditTransferWorkspacePage workspace={course} tab={tab ?? "chats"} />
  }

  if (course) {
    navigate({ ...chatLink(courseId, null), replace: true })
  }

  return (
    <div className="flex flex-col items-center justify-center h-[calc(100vh-200px)]">
      {courseLoading && <Skeleton className="h-10 w-40" />}
    </div>
  )
}
