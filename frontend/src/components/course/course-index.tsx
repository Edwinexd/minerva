import { useNavigate } from "@tanstack/react-router"
import { useQuery } from "@tanstack/react-query"
import { conversationsQuery, courseQuery } from "@/lib/queries"
import { Skeleton } from "@/components/ui/skeleton"

export function CourseIndex({ useParams }: { useParams: () => { courseId: string } }) {
  const { courseId } = useParams()
  const navigate = useNavigate()
  // Course load is the gate for the study-mode redirect: when the
  // course's `study_mode` flag is on, members never see the
  // conversation list; they're sent into the research pipeline at
  // `/course/{id}/study`. Wait for the course before acting on
  // conversations: that query can resolve first as `[]`, and
  // `else if (conversations)` is truthy on empty arrays, so without
  // this gate study-mode users get bounced to /new before the
  // study_mode check ever sees a non-undefined course.
  const { data: course, isLoading: courseLoading } = useQuery(courseQuery(courseId))
  const { data: conversations, isLoading: convLoading } = useQuery(conversationsQuery(courseId))

  if (course) {
    if (course.feature_flags?.study_mode === true) {
      navigate({
        to: "/course/$courseId/study",
        params: { courseId },
        replace: true,
      })
    } else if (conversations && conversations.length > 0) {
      navigate({
        to: "/course/$courseId/$conversationId",
        params: { courseId, conversationId: conversations[0].id },
        replace: true,
      })
    } else if (conversations) {
      navigate({
        to: "/course/$courseId/new",
        params: { courseId },
        replace: true,
      })
    }
  }

  return (
    <div className="flex flex-col items-center justify-center h-[calc(100vh-200px)]">
      {(courseLoading || convLoading) && <Skeleton className="h-10 w-40" />}
    </div>
  )
}
