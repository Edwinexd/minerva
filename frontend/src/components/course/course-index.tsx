import { useNavigate } from "@tanstack/react-router"
import { useQuery } from "@tanstack/react-query"
import { courseQuery } from "@/lib/queries"
import { Skeleton } from "@/components/ui/skeleton"

export function CourseIndex({ useParams }: { useParams: () => { courseId: string } }) {
  const { courseId } = useParams()
  const navigate = useNavigate()
  // Course load is the gate for the study-mode redirect: when the
  // course's `study_mode` flag is on, members never see the
  // conversation list; they're sent into the research pipeline at
  // `/course/{id}/study`. Everyone else lands on a fresh `/new`
  // chat. Previously this auto-selected the most recent
  // conversation, which polluted the context window any time a
  // student (or LTI launch) reopened the course. The history
  // sidebar is still one click away on the chat page itself.
  const { data: course, isLoading: courseLoading } = useQuery(courseQuery(courseId))

  if (course) {
    if (course.feature_flags?.study_mode === true) {
      navigate({
        to: "/course/$courseId/study",
        params: { courseId },
        replace: true,
      })
    } else {
      navigate({
        to: "/course/$courseId/new",
        params: { courseId },
        replace: true,
      })
    }
  }

  return (
    <div className="flex flex-col items-center justify-center h-[calc(100vh-200px)]">
      {courseLoading && <Skeleton className="h-10 w-40" />}
    </div>
  )
}
