import { useNavigate } from "@tanstack/react-router"
import { useQuery } from "@tanstack/react-query"
import { courseQuery } from "@/lib/queries"
import { Skeleton } from "@/components/ui/skeleton"

export function CourseIndex({ useParams }: { useParams: () => { courseId: string } }) {
  const { courseId } = useParams()
  const navigate = useNavigate()
  // Course load gates the redirect to a fresh `/new` chat. Previously
  // this auto-selected the most recent conversation, which polluted
  // the context window any time a student (or LTI launch) reopened the
  // course. The history sidebar is still one click away on the chat
  // page itself.
  const { data: course, isLoading: courseLoading } = useQuery(courseQuery(courseId))

  if (course) {
    navigate({
      to: "/course/$courseId/new",
      params: { courseId },
      replace: true,
    })
  }

  return (
    <div className="flex flex-col items-center justify-center h-[calc(100vh-200px)]">
      {courseLoading && <Skeleton className="h-10 w-40" />}
    </div>
  )
}
