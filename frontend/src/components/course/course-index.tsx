import { useNavigate } from "@tanstack/react-router"
import { useQuery } from "@tanstack/react-query"
import { conversationsQuery } from "@/lib/queries"
import { Skeleton } from "@/components/ui/skeleton"

export function CourseIndex({ useParams }: { useParams: () => { courseId: string } }) {
  const { courseId } = useParams()
  const navigate = useNavigate()
  const { data: conversations, isLoading } = useQuery(conversationsQuery(courseId))

  if (conversations && conversations.length > 0) {
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

  return (
    <div className="flex flex-col items-center justify-center h-[calc(100vh-200px)]">
      {isLoading && <Skeleton className="h-10 w-40" />}
    </div>
  )
}
